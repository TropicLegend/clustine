# Every service with several replicas: the plan, proposed

- Status: **Agreed by the owner on 2026-10-10** (the four questions that waited were answered; the others go by its defaults). Its first record is written: [ADR-0020](../adr/0020-one-stay-per-player.md). As proposed: Drafted on 2026-10-10 against `main` at
  `d836674`, by reading the code and the records; nothing was built or run for it.
  An independent reviewer went over the first draft against the code and found
  fourteen things; section 11 says what each changed. It waits for the owner's
  answers to the nine questions of section 8, each with a recommendation and what
  happens without an answer. Its first step is a decision record of its own for a
  player's place, which is reviewed before anything is built.

It rests on [replicas-and-availability.md](replicas-and-availability.md) ("the
groundwork", G §n) and does not repeat it. The decision records it names have the
numbers after those the plan for terrain names
([terrain-plan.md](terrain-plan.md)); whichever is written first takes the next
free number.

Marks: a `file:line` is something I read there, at `d836674`. **Inferred** is
reasoning from what I read and is a claim to test. **Guess** is from memory of
Kubernetes, Linux or the client, checked against nothing here.

Short names: `state.rs`, `service.rs`, `client.rs` are in `services/coordinator/src/`;
`fanout.rs` in `services/edge/src/`; `region.rs`, `api.rs` in
`crates/clustine-sim/src/`, and `sim/state.rs` is `state.rs` there; `messages.rs` in
`crates/clustine-rpc/src/`; `lanes.rs`, `disk.rs`, `local.rs` in
`services/worldstore/src/`; "the runner" is `services/worker/src/lib.rs`; `cluster/…`
is `bin/clustine/src/cluster/…`.

## 1. The short version

1. **Five phases** as before: R1 a player's place and one stay per player; R2
   several edges; R3 several coordinators; R4 balance by load, drains, numbers; R5
   the store survives its node. What changed is what R1 is and what comes before it.
2. **The world store becomes the one who knows each player's stay and place.** The
   first draft had the new edge ask every region to end the old stay. That is
   withdrawn: an edge is not linked to every region, nothing remembered the answer,
   and every login waited for every region (review, finding 1). Now every commit
   says where the players that changed are, the store keeps the latest place of
   the latest stay, and a login needs the home region and the store and nothing
   else. A stay that was replaced is dead at the store from that moment; regions
   and edges are told, and none of them is asked.
3. **This needs a decision record of its own, reviewed twice, before anything of R1
   is built** (ADR-0020). Section 4.1 is its starting point, not its text: I have
   not traced the edge's resume logic through it, and nothing was run.
4. **A join at a kept place does not go through the home region's land.** The home
   region gives the stay its number and lets it go to whoever holds the place in
   the same tick, without placing it first. Where nobody holds the place the home
   region takes it, as when somebody walks out today; R1.7 measures what that
   costs and section 4.1 says what follows from either result.
5. **Flying is kept** with the place. Nothing knows today that a player flies, and
   the owner's first trial of R1 is flown.
6. **"The store ratchets a term" is kept apart from "who elects".** The store gives
   out terms and refuses what is below one; who may ask for a term is behind one
   small interface. Its first form is a lease in the store, which the tests and a
   single machine can use; a Kubernetes Lease can take its place without touching
   the fences, and section 5 shows why it should once the store has a standby.
7. **Every wait is added up** (section 5). With the store electing, the loss of the
   node that has the store, the leader, a worker and an edge comes to about 16 of
   the edge's 20 seconds; with coordinators elected in Kubernetes, to about 10.
8. **Silence is three steps, not one**: a beat on the links of `clustine-rpc`, the
   same on the store's own sockets, and a deadline on every connect. Deadlines
   follow the lease.
9. **Balance**: the count of regions stays as the floor and tick time is added to
   it.
10. **Two questions are new and must not be acted on without the owner's answer**:
    a port of the kind cluster mapped to the host for real clients, which
    `edge.yaml` forbids in capitals; and a client for the Kubernetes API, which is
    a download.

## 2. Where the groundwork is out of date

It was written at `6f06cab`, before phase C. Checked against the code as it is:

| G § | It says | Now |
|---|---|---|
| head | ADR-0014 is proposed | C3 to C5 are built, and nothing the tests found is open (roadmap, "Where M3 stands") |
| all | `bin/clustine/src/cluster.rs` | A module: `cluster/{coordinator,edge,worker,worldstore,commands}.rs` |
| 1.1 | An edge waits for a worker for every region | Also for a table that names the home region, which only a coordinator that has read the store's list gives (`cluster/edge.rs:91-132`) |
| 1.3 (1) | Likeliest case: a player handed back and forth across a pinned stripe | Stripes are gone. Lands touch only with `--pin`, in the seconds a merge waits, or when the merge distance is too short (`cluster/coordinator.rs:56-63`). The gap is still there (`fanout.rs:410-424`, `1925-1929`) and has two new forms (ADR-0015, lines 376-379; ADR-0014, lines 2332-2340) |
| 1.3 (3) | ADR-0014 "proposed" orders two stays by entity id | Built: `region.rs:444-464`. A join replaces whatever stay the home region has (`region.rs:385-390`); it does not find a stay elsewhere |
| 1.3 "what it needs" | An edge links to every region | To every region that has an owner. A region without one has no route, and the table only counts such regions (`crates/clustine-region/src/lib.rs:29-43`) |
| 1.5 | Persist: "a join at a position in another region's chunk needs nothing new in the tick" (inferred) | Not so: the edge asks the home region for the whole view of whoever it places (`fanout.rs:2074-2075, 2080-2126`), and a viewer's subscription makes a region claim what nobody holds (`messages.rs:106-113`, `api.rs:419-427`). Section 4.1 |
| 2.3 | `Players` is ignored until C4 | Sent four times a second and used (`cluster/worker.rs:40`, `state.rs:1248`). Evening out picks the region with the fewest players (`state/follow.rs:1039-1061`). Workers are still weighed by count (`state.rs:2504-2516`) |
| 3.1, 3.2 | Regions from the layout | From the store's list only (`state.rs:926-932`, `cluster/coordinator.rs:83-94`), which also gives the highest epoch (`state.rs:1635-1637`) |
| 3.1 | "Workers notice at once (the connection ends)" | Only when the process dies. Nothing notices a peer that is silent (section 4.6) |
| 3.3 | A standby that answers `Refused` ends the worker | No such answer exists (`messages.rs:778-839`). A wrong coordinator's `Assigned` without a region makes the worker let go of it (`cluster/worker.rs:885-908`) |
| 3.3 | Two coordinators take regions from each other | And each would merge and split by itself, the default now (`bin/clustine/src/main.rs:107`) |
| 4.2 | All file access through `Disk` but `local::prepare` | Still so (`local.rs:37, 45, 123`). `Disk` has two methods more (`disk.rs:42-52, 82-85`, ADR-0018), and two threads write through it (`services/worldstore/src/lib.rs:132-146`) |
| 7.1 | Stale comment in `worker.yaml` | Fixed. Stale now: `deploy/kubernetes/kustomization.yaml:1` and `deploy/README.md:9` say two workers, `worker.yaml:49` has three |
| 8 | #1 waits for C3, #5 for C4 | Both done |
| 10 | `architecture.md` | Still "Replicated with embedded Raft, three replicas" (line 61) |

Still true and checked again: the edge stops at once on a signal
(`cluster/edge.rs:69-70`); a second connection of an account is refused
(`fanout.rs:875-878`); the player list and the count are each edge's own
(`fanout.rs:2041-2072, 2633-2648`, `services/edge/src/status.rs:44-55`);
`max_players` is enforced nowhere (`fanout.rs:122, 2905`); a join is a new entity at
the spawn point with the starting hotbar (`region.rs:391-407`), looking north
(`fanout.rs:2026-2027`), not flying (`fanout.rs:52`); the heartbeat carries no load
(`messages.rs:681`); nothing measures a tick (the runner:106-135); no service
authenticates another, and no greeting names a build
(`crates/clustine-rpc/src/tcp.rs:46-66`, `messages.rs:497-505`); the harness has one
coordinator and one edge (`bin/clustine/tests/common/processes.rs:27, 31`); the kind
cluster is one node and maps no port (`deploy/kind/cluster.yaml:7-14`).

## 3. What "several replicas" has to mean

| Service | Replicas | A process dies | Its node dies | Balanced by |
|---|---|---|---|---|
| Edge | n, all serving | Its players are disconnected, join through another edge and are back in place, flying if they flew. Others see them stand, go and come back | The same, noticed by the client itself, later (guess: 30 s) | New connections by whatever is in front; an edge that is full stops being ready |
| Coordinator | 2 or 3, one leads | Nothing, unless something else fails in the same seconds | The same, two seconds later | Nothing |
| Worker | n (today) | Its regions' players stand still 5 to 7 s (today) | The same | Count of regions, and tick time on top (new) |
| World store | 1 primary, 1 standby (R5) | Everybody stands still for seconds | The same (R5); today the world waits for the node | Nothing: one writer by design (G §4.3 D) |

**"Survives a node" means one node.** After it, the cluster is one short of
everything until somebody acts: Kubernetes does not replace a StatefulSet's pod on
a node it cannot reach, and a volume of kind's default class is one node's
directory (guess, to be tried in R0.6). The first minute is Clustine's own
noticing; Kubernetes notices later (guess: most of a minute).

**Which updates are rolling.** Messages are postcard, with no names or kinds on
the wire (`crates/clustine-rpc/src/wire.rs:24-36`, the runner:452-458), and a
region's stored state has a format number (the runner:459). An update is rolling
only between builds whose **wire number** (new, section 4.6) and state format are
equal. Any other update is: everything stopped, everything started. Everybody is
disconnected once and comes back in place, because the place no longer depends on
how a stay ended (4.1). This milestone changes the messages in every phase, so
between its phases the owner's updates are of the second kind.

## 4. Each service, from the code as it is

### 4.1 One stay per player, and a player's place (R1)

**This is a proposal for ADR-0020, which is the first step of R1 and is reviewed
twice before any type is fixed or anything is built.** The reason is in the review's
findings 1, 2, 4, 11 and 12: five orders of events broke the first draft, each found
by reading, and I have again only read.

What is there. A stay has a number that orders it: its entity id, which the home
region alone gives out, ascending (`region.rs:391-400`, `lanes.rs:1153-1167`). A
region's state has every player's pose, hotbar and slot, durable with every tick
(`sim/state.rs:32-47`), opaque to the store (`messages.rs:295-304`). Nothing
outlives the stay. Everything a region shows has passed through the store first
(ADR-0008, output commit), and one thread of the store decides everything
(ADR-0011 §6). That thread is the one place that can put the stays of a player in
one order.

**Who owns what.**

| | |
|---|---|
| The record's owner | The world store. One record per player |
| It carries | The stay (entity id); how often that stay was handed on (**hops**, new); position, look, on-ground; **flying** (new); hotbar; held slot |
| Who writes it | Whichever region has the stay, in the commit of every tick in which that player changed, in a part of the commit the store can read. The opaque state stays opaque; that the two say the same thing twice is accepted for now |
| What the store keeps | Of each player: the **floor**, which is the highest stay it was told the home region gave out; the record of that stay with the highest hops, the last written among equals; and where that stay was last said to be (a region, on its way, or ended) |
| Where it is durable | In the log with the commit that carried it, and in a file of the store's own before that log segment goes (`docs/world-format.md`, lines 157-164). Its format is the store's, written down in `world-format.md`, and does not hang on the state format |
| What is refused | A record of a stay below the floor is dropped. So a later place is never overwritten by an earlier one, whatever order the writes come in |

**The rules.**

1. **A login.** The edge ends any connection it has itself for that player, then
   sends the join to the home region, as today. The home region gives the stay its
   number in that tick and holds it as **entering**: not a player yet, shown to
   nobody. The commit of that tick tells the store.
2. **The store, on that commit**: raises the player's floor to the new stay; makes
   the record the new stay's, with the place it had; tells every region that has
   the region open that stays of this player below the floor are dead; and answers
   the home region with the place and with who holds the chunk of that place.
3. **When it answers.** If the old stay was last said to be in a region that is
   being run (it has the region open at the store), the store answers when that
   region's commit has removed the stay, or when that opening ends. In every other
   case it answers at once. **So a login never waits for a region that is away**:
   not for one without a worker, not for one being taken over, not for a stay on
   its way between two regions. It waits for the home region, as it does today, and
   for a region that is running, a tick or two.
4. **A region that is told a stay is dead** removes it in its next tick, drops what
   it has of it in any outbox, and tells the edge the stay belonged to, through that
   edge's outbox. A region names in its commit every stay that came into it in that
   tick (a join, an arrival, a merge, a split's part), and after it was opened,
   every stay it has. A stay named below its floor is answered "dead". A region
   that is opened is told the floors raised since its last commit before its first
   tick, and takes in no arrival below a floor it knows.
5. **An edge that is told its player's stay is dead** ends that connection, with the
   official server's sentence. An edge shows of one player only the highest stay
   it has seen: a lower one is hidden when a higher appears and never shown after.
6. **The home region, on the answer** (an input of a later tick, as chunks are, so
   that no other message of that edge waits behind the join, `sim/state.rs:60-61`):
   - No place: the player is placed at the spawn point, as today.
   - The place's chunk is another region's: the home region **lets the stay go to
     that region in this tick, without placing it**. The edge puts the player into
     the world when it hands them over, and asks the holder for what they see. The
     home region is asked for nothing and claims nothing.
   - Nobody holds the place, or the home region does: the player is placed there
     in the home region, which claims what they see.
7. **Every replacement tells the loser's edge**: a join over an entering or present
   stay (`region.rs:385-390`), an arrival over an earlier stay (`region.rs:444-464`)
   and a dead stay alike. Today the loser's edge is told nothing
   (`region.rs:694-702`) and its client stands in a dead world for 20 seconds
   (`fanout.rs:840-863`).
8. **What a stay did to another region's blocks names the stay** (`RemoteAction` and
   the word that it is done name only the player and the client's number today,
   `api.rs:197-204`, `fanout.rs:1747-1756`). A region drops an action of a stay it
   knows dead; an edge passes over a "done" of a stay it no longer has.
9. **The home region's next entity id never goes back**, also when its state is
   dropped for another build (the runner:3108-3113): the store says the highest
   stay it was told, and the region goes on above it. Without this a floor would
   be above every new stay.

**Every order the review lists:**

| Order of events | What happens |
|---|---|
| The old stay stands in a part that was split off a moment ago, which the new edge has no link to | No edge is asked. The place is the record. The part's worker is told through its opening at the store; until that opening it runs nothing that is shown (`cluster/worker.rs:95-103`), and its first commit names its stays |
| The old edge is alive (a second login from elsewhere) | The region that has the old stay removes it and tells the old edge, which ends the old connection. What the old client did in the tick or two before is applied, as before a kick |
| The old edge was frozen and wakes | What it kept and sends again: inputs of a stay no region has are passed over (`api.rs:380-381`); a kept arrival is refused by a region that knows the floor, or taken in, named and removed a tick later, and the edge is told; a kept action on blocks is dropped where the floor is known (rule 8) |
| A kept `PlayerArrive` lands after the new login | As above. The record is not touched: the stay is below the floor |
| A stay ends in the middle of a hand-over | The old region wrote the record in the tick that let the player go, with the place they were handed over at; a `Departed` is durable before anybody acts on it. `Discard` and `drop_edge` have nothing to write |
| A region is being taken over when somebody logs in | The login does not wait for it (rule 3). The place is that region's last commit, which is where the player stands, since the region has not ticked since. The new stay is handed to that region and waits there with everybody else of it; the old stay is removed in the region's first tick |
| Two logins within a few ticks, through two edges | The home region gives the numbers in the order it takes the joins. The lower stay is dead when the store takes the higher one's commit, wherever it has got to, and its edge is told (rule 4) |
| The store restarts between any two of these | Floors and records are on disk; regions name their stays again after opening |

**What "never twice" means here**, said plainly: after the new stay's first tick, no
region that runs ticks the old one beside it, except a region that takes in a kept
arrival whose floor it has not been told, for one or two ticks; no screen ever shows
both; the old stay can leave no trace in the record; its edge is told when it is
removed. A design in which the home region's commit waited for every other region
would be stricter and would make every login wait for the slowest region, which is
finding 1 (d).

**Flying.** Nothing knows that a player flies: the edge sends creative mode's
abilities without the flying bit (`fanout.rs:52, 2013-2017`; the bit is `0x02`), it
does not read the client's own word of it (no such packet in
`services/edge/src/play.rs`), and `Pose` has position, look and on-ground
(`api.rs:13-22`). Guess: placed in the air without the bit, the 26.3 client falls.
By the owner's rule this belongs in R1: the client's abilities packet read by the
edge, the flag in the sim's player, in the transfer and in the record, sent on
entering. The bots have no gravity, so only the comparison with the official server
and the owner can check it.

**A join at a kept place, and what the home region claims.** Rule 6 keeps the home
region out wherever a region holds the place: no rim of home land beside the
friend's region, no merge and split because of one. What is left is a place nobody
holds. Then the home region has a player far from the spawn point and is split, as
when that player first walked out. Two cases have to be measured in R1.7, with the
logs counted:

| Case | Asserted | If it is not so |
|---|---|---|
| Join inside a friend's land | No merge, no split, no chunk granted to the home region | A defect of R1 |
| Join 10 to 22 chunks from a friend (outside their land, inside the merge distance) | Measured: the friend's region is merged into the home region and both are split off again, as ADR-0017's open question 8 has it today | If the friend stands still twice: the store names as the receiver the region whose land is nearest within the merge distance, and that region claims the place. A change to rule 6 and to nothing else |
| Twenty join far apart at once (after an edge died) | Measured: how often and how long the home region's players stand still, against ADR-0017's half a second in the middle and a second at worst | If above: the home region lets each go into a part of their own in one split, which needs a split that names several parts; ADR-0016's open question 2 already asks for it |

**Not delegated**: the edge's part. **What the second review should attack**: the
store's wait in rule 3 against a region that is hung and not yet taken; a merge or a
split between a floor being raised and the region hearing; whether "every tick a
player changed" is too many bytes for a crowd (a hundred who walk are about 160 KB
a second, my arithmetic); the record of a player in a region whose state is dropped
for another build; an entering stay across a restore of the home region; how long a
region keeps a floor it was told.

### 4.2 Edges (R2)

What regions and the runner already do for several edges, and what they do not:

| | Evidence |
|---|---|
| State, outbox and numbering per edge; players name their edge | `sim/state.rs:27, 46, 51-66` |
| A leave or an input through another edge than the player's is passed over | `region.rs:432-435`; `api.rs:112, 376` |
| A later start of an edge resets it; an earlier one is turned away; an edge without a link for 600 ticks is gone | `region.rs:595-605`; the runner:2656, 71, 1743-1750 |
| An edge is told where its own players moved to, wherever its link watches | the runner:663-704, `region.rs:350-355` (new since the first draft) |
| The routing table goes to any number of edges | `service.rs:884-903` |
| Entity ids are unique across edges | `region.rs:391-400` |
| **Not**: order between regions for an edge that only watches | `fanout.rs:417-419`, `1925-1929` |
| **Not**: the source of an entity after a split, for other edges | ADR-0015, lines 376-379 |
| **Not**: an arrival that eats a later stay's inputs | ADR-0014, lines 2332-2340 |

**What is said of an entity, for an edge that only watches.** The hops of 4.1 order
introductions. Two of the three gaps are about removals and about absence, which
carry no state today (`api.rs:495-512`). The rule ADR-0020 has to state, and what
each case needs:

| Word | Rule | Closes |
|---|---|---|
| An introduction (a spawn, or a snapshot that has the entity) | Taken if its hops are at or above the highest the edge has heard of that entity; the region that said the highest is the entity's source. A hand-over raises the hops, and so does a split for those who go | A late snapshot of the old region; the source after a split |
| A move, a removal | They carry the hops. A removal at or above the highest heard removes the entity **and is remembered**: an introduction at or below it, read later, is passed over. `Discard` carries the hops of the transfer it gives up | A player who leaves in mid hand-over, where the removal is read before the old region's snapshot |
| Absence from a snapshot | Removes only what that region is the source of, as now | With the split raising hops, the part becomes the source at its first word |
| How long a removal is remembered | For the 30 seconds after which regions forget an edge, and never below the highest stay shown of that player (4.1, rule 5) | – |

Three tests in `fanout.rs`, one per row, each failing every time without its rule.

**What the edges share:**

| What | Today | Plan |
|---|---|---|
| One stay per player | Per edge only | Section 4.1 |
| Tab list, count, `max_players` | Each edge its own | A **roster** at the coordinator, in memory: each edge says whom it has, its public address and whether it drains, whole each time, over the connection it holds (`Role::Watcher` may say nothing today, `service.rs:569-574, 675-697`); every edge is sent the union. A new leader hears it again. Nothing that matters is decided by it |
| Chat | None | Out of scope |

**How a real client reaches a second edge.**

- **On one machine, as processes**: two edges on two ports, each started with its
  public address (`--public-address 127.0.0.1:25566`). The roster tells every edge
  the others' addresses, so a draining edge transfers its players to one that does
  not drain. After a `kill -9` the owner joins at the other port by hand.
- **On Kubernetes**: there is no way today. The Service is `ClusterIP` "and it has
  to stay that" (`deploy/kubernetes/edge.yaml:12-17`), the kind cluster maps no
  port on purpose (`deploy/kind/cluster.yaml:7-8`), and a `kubectl port-forward`
  goes to one pod and ends with it (guess). A client that loses its edge has lost
  its way in. What would do: in the overlay of the kind test only, a node port
  mapped to the host's loopback address and nowhere else, one address in front of
  all edges. That is the owner's question 7; without a yes, several edges on
  Kubernetes are tried by bots inside the cluster and by nobody else.

**A planned stop**: on the first signal the edge says in the roster that it drains
and stops being ready; it goes on taking connections for a setting's worth of
seconds, because whatever is in front sends it new ones for a moment yet (guess);
then for each player it says the leave, waits for the `Progress` that covers it
(`messages.rs:194-200`), sends the protocol's **transfer** packet, and closes. The
packet's ids are generated
(`crates/clustine-protocol/src/generated/packet_ids.rs:108, 427`) and the edge takes
intent 3 as a login (`services/edge/src/session.rs:57`); the packet is not written.
Where to: an edge of the roster that does not drain, or the one address in front.
So that a rollout transfers a player once and not once per edge, new edges come
before old ones go: edges become a Deployment that surges. A name per pod is
enough for an edge (`crates/clustine-world/src/position.rs:100-118`), and it ends
the question of a replacement's clock being behind (an edge's start is the wall
clock and a lower one is turned away, the runner:2656).

**An unplanned death** costs each of its players a "connection lost" and one click.
The join lands on a living edge and goes by 4.1: back in place as a new entity.
What was done and not confirmed is applied or not. Other players see the dead
edge's players stand still until they join again, or until the region forgets the
edge.

**Not delegated**: everything in `fanout.rs`.

### 4.3 Coordinator (R3)

**What the one that takes over has to know, and where it gets it.** Nothing from
disk, as today: the regions and the highest epoch from the store's list
(`state.rs:1627-1751`); who runs what from workers that register again with what
they hold (`cluster/worker.rs:1496-1548`, `state.rs:1086-1120`); a part nobody
heard of from its worker (`cluster/worker.rs:1524-1529`); where players are from
`Players` (`state.rs:1248`); the roster from the edges. Lost: releases, merges and
splits under way, which end by what the list shows (`state.rs:752-770`).

**Is "the highest epoch only" enough?** For the world, yes. Not for players, and
three things are not fenced:

| While two decide | What holds | What does not |
|---|---|---|
| Assigning, moving | Only the later epoch's owner gets anything confirmed (`lanes.rs:1141-1152`) | Regions go back and forth (G §3.3) |
| **The same epoch from both** | – | The store refuses only a lower epoch. Both raise their counter to what they were last shown and issue the next. Two workers then hold one region with one epoch, each admitted in turn, each opening again with the same hello (`cluster/worker.rs:1257-1285`): for ever |
| Merge, split | One record each; the store wants the absorbed region's epoch (`messages.rs:333-340`) and the next id (`messages.rs:447-450`) | Both begin them by themselves |
| Routing table | Nothing | An edge follows its coordinator's table (`cluster/edge.rs:258-284`) |
| Orders | Nothing | A stale `Assigned` makes a worker let go (`cluster/worker.rs:885-908`) |

**Two things, kept apart.**

*The fence is the store's, whoever elects.* The store keeps a **term** on disk and
only ever raises it:

1. **It gives terms out.** Whoever may lead asks the store for a term and is given
   one above every term the store has given or been shown. It is the high part of
   every epoch that leader issues, above any epoch a world has today (those are
   wall-clock milliseconds, about 2^41: `service.rs:104-107, 267-274`). Two leaders
   can never issue one epoch, and the clock leaves the epochs.
2. **It refuses what would move a region under an earlier term**: a hello that
   **raises** a region's epoch has to be of the current term, and so has the epoch
   a merge or a split names. A hello with the epoch a region already has is
   admitted as now, whatever the term: it is the same owner opening again
   (`cluster/worker.rs:1257-1285`).
3. **A refusal says the term.** `EpochRefused` says the region's highest epoch
   today (`messages.rs:682-685`), which is of the old term and tells a deposed
   leader nothing.
4. **The store's answers to workers carry the term**, so every worker knows a new
   term within a tick, and a worker's links tell its edges. A worker and an edge
   pass over an order or a table of a lower term than they know, and leave a
   coordinator of one.
5. **A coordinator issues only epochs of its own term, and a later term anywhere
   deposes it.** Four places raise the counter to what they are shown today
   (`state.rs:1283, 1514, 1636, 2860`); each becomes "of a later term: I am
   deposed", and a deposed coordinator closes every connection.
6. **A store restored from a backup** has an earlier term than workers and edges
   have seen. A leader that is shown a later term than its own says so when it
   asks for the next, and the store goes above it. The README says that a restore
   means stopping everything and starting it anew: workers hold states the store
   no longer has.

*Who may ask for a term is behind one interface*, with three forms:

| Elector | Where | When |
|---|---|---|
| A lease in the world store: one holder at a time, the holder and the term on disk, renewals in memory | Plain processes, every test, Kubernetes until R5 | R3 |
| Nobody else: the single process (`Coordinator::alone`, `service.rs:133-135`), which asks its own store for a term at every start and has no grace, as now | One machine | R3 |
| A Kubernetes Lease | Kubernetes, once the store has a standby, if the owner allows the client (question 8) | R5 |

Why the store's lease first: the tests that start clusters of processes can elect
with it, and it needs no download. Why not only it: it ties leading to the store
being there, and section 5 shows what that costs when the store fails over.

**What a silent store may and may not make the coordinator do.**

| | |
|---|---|
| May | Stop deciding while it cannot renew: no assignment, no move, no merge, no split, no evening out. Go on in **its own term, without a grace**, when the store answers again and still names it as the holder |
| May not | Take a region from a worker for want of vouching while it cannot reach the store itself; mark a worker at fault for it; close its clients' connections; give up its term by itself |
| How the store allows it | The holder is on disk with the term. A store that starts, or a standby that is promoted, lets only that holder renew for one lease's time before anybody else may ask |

Today a store that is killed and started costs the coordinator nothing
(`cluster/worker.rs:1257-1285`, `state.rs:913`); this keeps it so. The first draft's
"closes every connection" when it cannot renew is withdrawn.

**Finding the leader.** Every client takes one address now
(`bin/clustine/src/main.rs:272, 300, 318, 335, 363`). It becomes a list. All of it
is tried at once, each connect with a deadline; a standby answers "not the leader,
it is there" and closes. "Ready" stays "it listens".

**What a player notices when it changes**: nothing. Section 5 has the times.

**What the reviewer should attack**: the four places; a leader frozen for longer
than its lease that wakes and gives one more order; the holder on disk across a
promotion; epochs from before terms, which must depose nobody; the version of the
routing table, which starts from the first epoch (`state.rs:953-955`).

### 4.4 Workers, and balance by load (R4)

Workers are several already (G §2.2). Missing:

**Measured.** By the runner, per region, over a window of some seconds: processor
time of the region's thread per tick, and beside it the time by the wall clock,
which also grows when the machine is oversubscribed; ticks skipped (the
runner:2274-2276 skips without counting); players; loaded chunks. Reported with the
heartbeat (`messages.rs:681`), with the worker's processors as its manifest limits
them.

**The weight.** A region weighs **one, plus** its share of a tick. A worker's load
is the sum over its regions. The count stays as the floor: it is what spreads
regions over a worker that joins, brings them back after every worker was replaced
in turn, and bounds how many regions stand still together when one worker dies
(`state.rs:2385-2442, 2504-2516, 2546-2560`). With tens of players a region's tick
is a small part of 50 ms, and weights alone would move nothing.

**Who acts, and when.** The coordinator, in `lightest` and `even_out`:

| Rule | Why |
|---|---|
| Load never merges or splits | Distance stays the only reason (ADR-0016); a region is one thread and no move helps it |
| The trigger is the difference between the heaviest and the lightest worker standing above a threshold for some seconds. The threshold is two, as today's "difference of two" is | With nobody building, this is exactly today's rule |
| The region moved is the one that leaves the two closest without crossing | A small region moves; twenty parts made on one worker (`state.rs:2391-2392`) are evened out one after the other |
| A region is not judged before a whole window after its rest | Its time on the worker it left says nothing, and the ticks after a restore are its worst |
| Everything else as `even_out` has it | One release at a time, never in the grace period, never a worker at fault, never a region that rests or is wanted (`state/follow.rs:1039-1061`) |

**ADR-0017's open question 2** (who walks away stands still twice, for the split
and for the move ten seconds later) does not fall out of this, as the first draft
said: the worker of the home region is the heavier because of the home region.
ADR-0025 answers it by a rule of its own; it is the owner's question 9.

**A planned stop of a node.** The pieces exist (`cluster/worker.rs:700-705`,
`LEAVE_WITHIN` at line 81, `worker.yaml:78`). Missing: budgets so that a drain
takes one worker and one edge at a time; the edge's drain (4.2); a test with many
regions on a worker that leaves while regions merge and split (roadmap, "not
tried" under C4 and C5). **The node that has the store is not drained before R5.**

**Scaling.** By hand, as today. This milestone exports the numbers (`clustine
status`, and a plain-text endpoint in Prometheus' format, written by hand) and
leaves the autoscaler and the operator to M4.

### 4.5 World store (R5, and guards earlier)

| Option | Changes to the store's promises | Everybody stands still | The owner provides | Verdict |
|---|---|---|---|---|
| **A. A volume that follows the pod** (G §4.3 A) | None in the code. ADR-0008's "answered once synced" rests on the volume: a sync must not return before the replicas have it, a volume attached anew must show every acknowledged write, and one node only may have it | Node dead: until Kubernetes lets the pod go (guess: minutes). Node drained: above 20 s everybody is disconnected | Replicated block storage and node fencing | Cannot be shown on kind |
| **B. A standby that mirrors the disk** (G §4.3 B) | ADR-0008: a commit is answered when both have synced it, or the primary holds a third party's word that it runs alone. ADR-0011 and ADR-0018 unchanged above `Disk` | Section 5 | A second volume; in Kubernetes a Lease | **Recommended, last** |
| A standby that receives the log | A second recovery path (`messages.rs:286-309`) | – | – | No |
| Several stores | Undoes ADR-0011; a merge would be a transaction on the ordinary path | No better | – | No |
| Raft; an external database | – | – | – | No (G §4.3 C, E) |

**Why B, and why last**: as in the first draft. It is the only one the kind test and
the owner can see, the only one under which draining the store's node is a pause,
and it reuses the one recovery path: the simulated disk already has the crash that
keeps everything written (`disk.rs:352-366`), which is what a standby holds. It is
three to six weeks by the groundwork's measure, in the one part where a mistake is
not a pause.

**Built early whatever is chosen** (R3.7): one store per directory (there is no
lock in `services/worldstore`); `clustine backup`; what a storage class has to
promise, in the README.

**Who may promote**: a Kubernetes Lease that records the primary, its term, and
whether the standby is in step, changed only by compare-and-set. A primary that
loses its standby writes "alone" there before it answers any commit made alone; a
standby promotes only if the Lease says it was in step. Outside Kubernetes, by
hand. This does not rest on clocks for safety; clocks decide how long it takes.

**What ADR-0026's reviews should attack**, the first draft's list and the review's:

- A primary that answers a commit the standby does not have; a standby promoted
  while the primary still answers; the old primary coming back.
- **Two threads write through one `Disk`** and hand files to each other
  (`services/worldstore/src/lib.rs:132-146`). One stream in the order the calls
  return is enough, but then a commit's sync waits behind a checkpoint's round on
  the standby, the wait ADR-0018 took away on the primary. The record chooses and
  says what it costs.
- **A write that fails on one side only** (`disk.rs:336-350` puts half in place):
  the two differ until the store's own cutting back is mirrored. A test by name.
- **A primary that ran alone and is lost for good**: the standby may not promote
  by the rule above. There has to be a hand that says "promote, and lose what was
  done alone", and then everything is started anew.
- **A standby begins by copying, never by preparing**: `local::prepare` does not
  go through `Disk` (`local.rs:37, 45, 123`).
- The rounds of ADR-0018 arriving in part; bringing a standby up to date while the
  primary runs; the division the standby is started with
  (`cluster/worldstore.rs:17-27`); the player records and the term, which are files
  like the others.

### 4.6 Between services: greetings, silence, authentication

**The greeting, changed once** (R0.3). Three doors begin three ways today
(`crates/clustine-rpc/src/tcp.rs:46-66`, the coordinator's first message, the
store's `StoreHello`). One greeting for all three, which names:

| | Why |
|---|---|
| A **wire number**, raised with every change to the shape of a message, held by a test of known bytes as the state format is (the runner:6386-6394) | Bytes of one shape read as another. A different number is refused loudly, with both numbers |
| The role and the name | The coordinator takes the first message for the role today |
| The term the speaker knows | 4.3, fence 4 |
| The interval of the beat | Below |
| Room for a challenge and its proof | Authentication, if the owner says so, without changing the greeting again |

**A worker of another state format.** Today it starts a region whose state it
cannot read as one that has never run (the runner:3108-3113), and the edge puts
the players out with "The server lost track of where you are" (`fanout.rs:1680`).
With the wire number no edge of the old build is linked to such a worker, so this
happens only in an update that is not rolling. Then it may do exactly that: the
players' records and the entity ids are the store's and survive (4.1, rule 9), and
everybody comes back in place. It may not: take a state of a newer format for its
own (it fails today, the runner:481-482; it should refuse the region and say so,
and the coordinator should not count that as a fault), or give out an entity id
below what the store was told.

**Silence.** No keep-alive, no user timeout and no read deadline is set on any
connection between services but in the store's greeting
(`services/worldstore/src/tcp.rs:533-536`). A connection that only reads never
notices: the edge's watch (`cluster/edge.rs:413-416`). One that writes notices
when its queue is full: a worker after 256 heartbeats (`client.rs:410-450`,
`services/coordinator/src/lib.rs:36`), a region after 16,384 messages to an edge
(`bin/clustine/src/lib.rs:41`, the runner:3033), and only then begin its 600
ticks. Two connects have no deadline (`crates/clustine-rpc/src/tcp.rs:55`,
`client.rs:664`); guess: to a node that is gone they last about two minutes.

Three steps, not one:

| Step | Where | Note |
|---|---|---|
| A beat both ways, and a link that hears nothing for the deadline is ended | `crates/clustine-rpc/src/link.rs`: the coordinator's connections and the region links | Answered by the task that reads and writes the socket, never by the fan-out task: a busy peer is not a dead one |
| The same on the store's blocking sockets | `services/worldstore/src/tcp.rs`, which is R1.1's crate: before R1.1, by the same hands | Answered by the socket's thread, never the commit thread |
| A deadline on every connect | The two above and the store's | A third kind of peer in the tests, besides killed and frozen: one that never answers a connect |

The deadlines are in section 5.

**Authentication**: the small form (G §6, 1 and 2), as a side step, if the owner
says so (question 3). The reason is new: whoever reaches the store's port can ask
for a term. No download: a keyed hash is in the workspace (`Cargo.toml:29`).

## 5. Every wait, added up

**The deadlines I propose, and what each follows from.** They follow the worker
lease, which is a setting (5 s; 3 at the least, `bin/clustine/src/main.rs:221`).

| | At a lease of 5 s | Follows from |
|---|---|---|
| Beat | 0.5 s | A quarter of the shortest deadline |
| A link to the **store** silent | 2 s (two fifths of the lease) | A region whose commits are not confirmed stops after eight ticks (the runner:82) and is then vouched for by nobody (`cluster/worker.rs:45, 127-143`); `WaitingForStore`, which the coordinator bears for 30 s (`state.rs:913`), is said only when the handle is known lost (`cluster/worker.rs:1257-1285`). So the handle has to be known lost within the lease less a heartbeat (1 s, `client.rs:28`) less a look (0.25 s, `cluster/worker.rs:40`): 3.75 s. At a lease of 3 s: 1.2 against 1.75 |
| A link to the **coordinator** silent | 2 s | Noticing and finding the next leader has to fit inside the leader's lease and the new leader's grace, or living workers lose their regions |
| A link between a region and an **edge** silent | 5 s | It only begins the 600 ticks (the runner:71) |
| A connect | 1 s | One network; the edge's is 2 s today (`cluster/edge.rs:39`) |
| The leader's lease | 3 s, renewed every second | Below the worker lease |
| The store primary's Lease (R5) | 4 s | Above the store's silence deadline; Kubernetes' usual client waits 15 (guess) and has to be told |
| The new leader's grace | 5 s: one worker lease, as now (`state.rs:843-847`) | Workers have to find it and say what they hold |
| Promotion of a standby store | Aim: 2 s. Not known | Opening a directory as after a crash; measured in R5.1 |
| The edge gives a player up | 20 s (`services/edge/src/lib.rs:81`) | Unchanged |

**A process that is killed** is heard at once; subtract every "silent" row.

**The node that has the store, the leader, a worker and an edge is gone**, for the
worst-placed player who stays connected (another edge; their region was on the dead
worker). Waits in a row:

| # | Wait | The store elects (R3 as built) | A Kubernetes Lease elects (R5) |
|---|---|---|---|
| 1 | The standby store knows: the longer of its silence deadline and the Lease | 4 s | 4 s |
| 2 | Promotion | 2 s → at 6 | 2 s → at 6 |
| 3 | A coordinator may lead | It finds the new primary (1 s), which gives the old holder one lease's time (3 s) → at 10 | Its Lease ran out beside row 1; it asks the store for a term → at 7 |
| 4 | The grace | 5 s → at 15 | Counted from when it was elected and began to listen (4 s) → at 9 |
| 5 | The dead worker's regions assigned, opened, restored, resumed | 1 s → **16 s** | 1 s → **10 s** |
| | Left of the edge's 20 s | 4 s | 10 s |

Players whose region is on a worker that lives: they stand still until the store is
promoted and their worker has opened the region again: about 7.5 s either way.
Their regions are said to wait for the store from second 2 on. Players of the dead
edge are disconnected and find out by their client (guess: up to 30 s).

**So**: four seconds are not a margin. Once the store has a standby, coordinators in
Kubernetes should be elected by the Kubernetes Lease, which the fences do not
notice (4.3). Outside Kubernetes a standby store is promoted by hand, and the row
does not apply.

**Other losses** (silent; a killed process is two seconds sooner):

| Lost | A player notices | After |
|---|---|---|
| A worker's node | Its regions stand still | 5 to 7 s, as today |
| The leader's node | Nothing. Nothing is taken over, merged or split meanwhile | 3 s of lease and 5 of grace |
| The leader and a worker | That worker's regions stand still | About 9 s |
| The store's node (R5) | Everybody stands still | About 7.5 s |
| The store's process, restarted | Everybody stands still; the leader goes on in its term | As long as the restart, as today |
| An edge's node | Its players are disconnected | Their client's own time |

## 6. The phases

Names are mine. Sizes: **S** one file and its tests, some hundreds of lines; **M**
one crate, up to about 1,500 lines with tests; **L** several crates, or more, or a
record with its reviews; **XL** more than one L. They come from what each step
touches, with the lines of the files as they are: `fanout.rs` 14,266, the runner
13,072, `state.rs` 9,194, `region.rs` 6,482, `service.rs` 3,543,
`cluster/worker.rs` 2,882, the store's `tcp.rs` 2,090, `lanes.rs` 1,982,
`ledger.rs` 1,948, `client.rs` 1,609, `chaos.rs` 1,582, `messages.rs` 839,
`link.rs` 590, `bin/clustine/src/lib.rs` 444. About three quarters of each large
file are its tests.

### What has to be reviewed before which step

| Before | Reviewed |
|---|---|
| R0.3 (greetings) | ADR-0024, whole: the greeting, the term, the elector's interface, the holder on disk, the deadlines of section 5 |
| R1.0b (the types of R1) and everything of R1 | ADR-0020, **twice** |
| R2.1 | ADR-0020's part on entities, which the second review covers |
| R3.1 | ADR-0024, as above. What a promotion does to the term and the holder is in it, so that R5 does not undo R3 |
| R4.2 | ADR-0025 |
| R5.1 | ADR-0026, twice |

### R0: what everything stands on

| # | Scope | Size | Verified by | Who |
|---|---|---|---|---|
| R0.1 | This plan agreed; the owner's questions asked | S | – | main session |
| R0.2 | ADR-0024 written and reviewed. ADR-0020 written and reviewed the first time | L | The reviews | main session; 0020 drafted by a subagent |
| R0.3 | The greeting, once, at all three doors, with the wire number; refused or passed over by everybody; pushed | M | All existing tests; a build with another number is refused with both numbers | main session |
| R0.4a | The beat on the links of `clustine-rpc` | S | A peer stopped with `SIGSTOP` is given up within the deadline | subagent, `clustine-rpc` |
| R0.4b | The beat on the store's sockets | M | The same | subagent, `services/worldstore`: the one who then has R1.1 |
| R0.4c | A deadline on every connect | S | A peer that never answers a connect | subagent |
| R0.5 | The harness starts several coordinators and edges, freezes any process, and has a peer that never answers; the bots leave and join again | M | The existing tests | subagent, `bin/clustine/tests/common`, `tools/botswarm` |
| R0.6 | Trials, thrown away: the coordinator, the store and an edge frozen under bots; on kind, what a stopped node does to a Service, a headless name, a pod and a volume | S | What was found, in the roadmap | subagent; kind's part on the owner's machine |

### R1: one stay per player, and a player's place

| # | Scope | Size | Verified by | Who |
|---|---|---|---|---|
| R1.0a | ADR-0020 reviewed the second time | L | The review | main session |
| R1.0b | Its types, in `clustine-rpc` and `clustine-sim`; the wire number and the state format raised; pushed | M | All existing tests | main session |
| R1.1 | Store: records, floors, where a stay is, the answers of rules 2 to 4, the file and its format | L | The store killed at every write; scenarios from the record by another | subagent, `services/worldstore` |
| R1.2 | Sim: hops; entering; what a commit says of stays; dead stays; flying; actions that name their stay; every replacement tells the loser's edge | L | Scenario and differential tests from the record by another | subagent, `crates/clustine-sim` |
| R1.3 | Runner: stays into the commit, the store's answers into ticks, the entity id from the store | M | Runner tests from the record, the store lost at each point | subagent, `services/worker` |
| R1.4 | Edge: the new login wins; entering by being let go; a stay that is dead; the look and flying at entering, the client's abilities read; the highest stay only on a screen | L | The edge against scripted regions | main session, **not delegated** |
| R1.5 | The single process (`bin/clustine/src/lib.rs`) on all of it | S | `single.rs`, `persistence.rs` | main session |
| R1.6 | The bots across a leave: place, look, hotbar, slot; the comparisons with the official server for entering with a look and flying | M | `--ignored` tests with the jar | subagent, `tools/botswarm` |
| R1.7 | End to end, in the single process and in a cluster **with two edges**: every row of 4.1's table by name (the other edge alive, dead, frozen and woken; a part split a moment ago; a kept arrival; a leave in mid hand-over; a region taken over at the login; two logins at once); the three cases of the join at a place, measured; a build with another state format | L | Tests from the record by someone who saw none of R1.1 to R1.5 | subagent |
| R1.8 | Docs; what to try | S | CI | main session |

**The owner tries** (single process, one client, then two): fly out, look at
something, change the held slot, leave, join: there, in the air, flying, looking at
it, holding it. Stop the server and start it: the same. Join as the same name from
a second client: the first is put out with a sentence, the second stands where the
first stood. Two friends out there, one leaves and joins: the other's screen shows
them go and come, and the log has no merge and no split.

### R2: several edges

| # | Scope | Size | Verified by | Who |
|---|---|---|---|---|
| R2.1 | What is said of an entity, for an edge that watches (4.2); ADR-0014's note on arrivals and inputs | L | Three tests in `fanout.rs`, one per case, each failing every time without its rule; scenarios from the record | sim and runner: subagents (S each); edge: **not delegated** |
| R2.2 | The roster | M | State and service tests; two bots on two edges see one list | coordinator: subagent, after R3.2; edge: main session |
| R2.3 | Every hand-over, block, takeover, merge and wander test with the watcher on another edge | M | Those tests | subagent |
| R2.4 | Chaos: one edge of two killed, and frozen and woken, at logged moments | L | The ledger of section 7 | subagent who saw none of R2.1 |
| R2.5 | The transfer packet in the codec and in the bots; an edge that drains | M | The comparison with the official server if its `/transfer` can be driven; the edges replaced in turn under bots that follow | packet and bots: subagent; drain: **not delegated** |
| R2.6 | Manifests: edges as a Deployment that surges, apart, with time to drain; kind: an edge's pod deleted, and the edges rolled out, under bots | M | `deploy/kind/test.sh` | subagent, `deploy/` |
| R2.7 | Docs; what to try | S | CI | main session |

**The owner tries** (processes, two edges on two ports): each client on an edge;
`kill -9` one edge, join at the other port, stand where you stood. Ctrl-C an edge:
its client is moved over by itself. A real client is needed for the transfer.

### R3: several coordinators

R3.1 to R3.3 and R3.5 to R3.7 are built beside R1 and R2 by other hands.

| # | Scope | Size | Verified by | Who |
|---|---|---|---|---|
| R3.1 | Store: the term, given out and never lowered; the lease and its holder on disk; refusals that say the term; the term in answers | M | Store tests, killed at every write of a term or a holder | subagent, `services/worldstore`, after R1.1 |
| R3.2 | Coordinator: epochs of its term; the four places; stops deciding; what a silent store may not make it do | L | The state machine's tests and a set from the record; faults put in, as for C4 | subagent, `services/coordinator` |
| R3.3 | Service and client: the elector's interface with its first two forms; a list tried at once; "not the leader" | M | Service tests over sockets | the same subagent, after R3.2 |
| R3.4 | Worker and edge know the term and leave a coordinator of a lower one; every process takes a list; the single process asks for its term | M | `cluster/` tests; `single.rs` | main session |
| R3.5 | Chaos: two started at once; the leader killed, frozen and woken, alone and with a worker, at logged moments of a move, a merge and a split; the store killed while a leader leads, which must cost no leader | L | Tests from the record by another | subagent |
| R3.6 | Manifests: three coordinators apart, a headless Service; kind: the leader's pod deleted while a group walks out | M | `deploy/kind/test.sh` | subagent, `deploy/` |
| R3.7 | One store per directory; `clustine backup`; what a restore means; what a storage class has to promise | M | Tests; the README | subagent |

**The owner tries**: three coordinators; kill the leader while flying out: the split
comes about eight seconds late and nothing else shows. Kill the store and start
it: the leader's log says it goes on in its term.

### R4: balance by load, drains, numbers

| # | Scope | Size | Verified by | Who |
|---|---|---|---|---|
| R4.0 | ADR-0025, reviewed | M | – | subagent drafts |
| R4.1 | Runner measures; the heartbeat carries it | S | Runner tests | subagent |
| R4.2 | The weight in `lightest` and `even_out` | L | Scripted loads; generated runs held to ADR-0016's seven properties and to: nothing moves back and forth, a worker that joins gets regions, every worker replaced in turn ends spread, with no load it is today's rule | subagent; the generated runs by another |
| R4.3 | `clustine status`; numbers in Prometheus' format | S | Tests | subagent |
| R4.4 | Budgets and anti-affinity; a worker with many regions told to stop while regions merge and split; kind: a node drained that does not have the store | M | `moves.rs`, `wanders.rs`, `deploy/kind/test.sh` | subagent |
| R4.5 | Authentication, if the owner says so | M | A connection without the secret turned away at each door | subagent |

### R5: the store survives its node

| # | Scope | Size | Verified by | Who |
|---|---|---|---|---|
| R5.0 | ADR-0026, written during R1 to R3, reviewed twice | L | – | main session |
| R5.1 | A `Disk` that mirrors, and the standby that receives | L | The kill tests on two simulated disks and a connection, a fault at every operation of either and every message between, one side failing alone by name; a store opened on the standby's image restores everything confirmed | subagent; scenarios by another |
| R5.2 | Bringing a standby up to date while the primary runs | L | The same, begun at every point | subagent |
| R5.3 | Who may promote: the Lease, "alone", the hand; a client for the Kubernetes API (question 8) | M | – | main session, **not delegated** |
| R5.4 | Workers and coordinators find the primary; coordinators elected by the Kubernetes Lease there | M | `cluster/` tests | subagent |
| R5.5 | Chaos: the primary killed and frozen at logged moments; the standby killed; both in turn | L | The ledger; nobody disconnected; the times of section 5 | subagent who saw none of it |
| R5.6 | kind with several nodes: a node stopped that has the primary, an edge, a worker and the leader | M | `deploy/kind/test.sh` | subagent |

### The critical path

Everything that is not delegated, and the reading of everything that comes back,
is one queue: R0.1, R0.2, R0.3, R1.0a, R1.0b, R1.4, R1.5, R2.1's edge, R2.2's edge,
R2.5's drain, R3.4, R5.0, R5.3. Subagents wait on that queue, not on each other.
Side by side in crates of their own: R0.4a, R0.5, R1.1 (after R0.4b, same hands),
R1.2, R1.3, R3.2. One after the other in one crate: R1.1 then R3.1 in the store;
R3.2 then R2.2's and R4.2's parts in `state.rs`.

## 7. How it is tested

**The ledger** (`tools/botswarm/src/ledger.rs:12-27`) fails today if anybody is
disconnected. It becomes:

- A bot may be disconnected only if its edge was killed, frozen or drained at a
  moment the test logged, and joins again within a bound.
- After it: it stands where the server last had it, flying if it flew; hotbar and
  slot are what the server last said.
- An action sent and not acknowledged when the connection was lost is **in doubt
  until the old edge is gone**: until every region has forgotten it or it was
  killed. Until then the block may be as before or as after, and may still change.
  After that it is what the bot is shown.
- "No entity vanishes unless far away" has one exception: the entities of such an
  edge's bots, between the loss and their return. **No bot is ever shown two
  entities of one player** stays without exception; the edge's rule in 4.1 is what
  keeps it.
- After a transfer nothing is in doubt.

**Three kinds of loss**: killed (`kill -9`), frozen (`SIGSTOP`, which only workers
meet today, `chaos.rs:710-715`), and a peer that never answers a connect. The
third is the nearest one machine comes to a node that is gone; the kind test is
the only place a node really goes.

**Rolling**: one test rolls between two builds with different wire numbers and
asserts the loud refusal and that every bot is back in place.

**The kind test** gets nodes and shows, under the ledger bots: an edge's pod
deleted, the edges rolled out (R2); the leader's pod deleted (R3); a node drained
(R4); a node stopped (R5).

**What one machine cannot show**: a real replicated volume; a load balancer in
front of the edges; the cost of a mirrored sync over a network; whether six
processors carry a kind cluster of four nodes beside the bots; a real client on a
transfer, on a lost connection and in the air.

## 8. Questions that are the owner's

**Not to be acted on without an answer**: 7 and 8. Everything else has a default
that can be taken back.

| # | Question | Recommended | Without an answer |
|---|---|---|---|
| 1 | May the store's survival of a node rest on replicated storage that whoever runs the cluster provides, or is Clustine to carry a standby of its own? | A standby of its own, last | R0 to R4 are built; ADR-0026 is written and reviewed; the standby is built last unless the owner says a volume is enough |
| 2 | When the store fails over, everybody stands still, and the edge disconnects them after 20 s. Keep 20? | Keep; section 5 aims at 10 | Kept |
| 3 | Authentication between services here: NetworkPolicies and a shared secret? | Yes | The policies are written and the greeting has room; the secret is built in R4 |
| 4 | A second login puts the first out, as the official server does | Yes | Built so |
| 5 | The autoscaler and the operator stay in M4 | Yes | So |
| 6 | The kind test grows to three or four nodes and stops one with `docker stop` | Three | Three; the R5 part can be skipped by a setting |
| 7 | **How do real clients reach several edges on the kind cluster?** A node port mapped to the host's loopback address only, in the test's overlay only, would give one address in front of all edges. `edge.yaml` forbids a node port in capitals | Yes, loopback only | **Nothing is mapped.** The owner tries several edges as processes on two ports; on Kubernetes only bots do |
| 8 | **A client for the Kubernetes API** is needed for R5's Lease and for electing coordinators in Kubernetes. It is a download and a library to evaluate, or a small client of our own over what is in the lock file | Evaluate one crate, ask again with what was found | **Nothing is downloaded.** R5.3 and R5.4 wait; outside them the plan needs none |
| 9 | Who walks away from the others stands still twice in ten seconds in a cluster, for the split and for the move (ADR-0017, open question 2). Leave a part where it was made while its worker is no more than one region ahead? | Yes | As today |

One thing is not a question and the owner should know it: from R1 on a world
directory has files an earlier build does not know. Copy a world before its first
run on R1; `clustine backup` comes with R3.7.

**The groundwork's twelve:**

| G §9 | |
|---|---|
| 1 What is survived | Answered: a process at least, preferably a node |
| 2 Depending on something outside | Answered in part: the Kubernetes API, yes. Storage: question 1. A client crate: question 8 |
| 3 A disconnect when an edge dies | Answered |
| 4 Place and hotbar | Answered: part of this. Where: the store (4.1) |
| 5 Second login | Open: question 4 |
| 6 How many players | Open, not asked: tens of players, a handful of each service |
| 7 Without Kubernetes | Settled by the plan: coordinators and edges yes; a store standby is promoted by hand there |
| 8 Edges on the internet | No. But how a real client reaches two edges is question 7 |
| 9 Authentication | Open: question 3 |
| 10 The store's pause | Open: questions 1 and 2 |
| 11 The lease of 5 s | Moot: a setting; the deadlines follow it |
| 12 How the owner tries it | Open: questions 6 and 7 |

## 9. Risks, and what is out of scope

**Risks.**

- **Section 4.1 is new and read, not run.** It moves a duty into the store's one
  thread, on the path of every commit. Two reviews are planned for that reason.
- **Every commit grows** by the players that changed. Fine for tens; a question
  for a crowd.
- **The home region is where everybody enters.** A place nobody holds makes it
  claim land there. R1.7 measures it; the remedies are named and not built.
- **1,048,576 stays for the life of a world**
  (`crates/clustine-world/src/position.rs:133`; past them a join is refused,
  `region.rs:392-398`). Every join, transfer and rejoin uses one.
- **Four seconds under the edge's twenty** until coordinators are elected in
  Kubernetes (section 5), and the promotion time is a guess.
- **Two electors** in the end, behind one interface. More to test.
- **A lease is only as good as clocks that run at the same speed.** The fences of
  4.3 are what hold when one does not; R3.5 has to show it.
- **The store's standby** can lose what players were shown if "who may promote" is
  wrong.
- **Time.** The cluster tests add three quarters of an hour already (roadmap, line
  738), the Cluster workflow has 45 minutes (`.github/workflows/cluster.yml:29`).
- **The owner's trial of C5 is outstanding**; what it finds lands in `fanout.rs`,
  which R1.4 changes. The terrain milestone changes the store beside R1.1 and R3.1.
- **This machine's memory** has been defective (`CLAUDE.md`).

**Out of scope.** Online mode, encryption, an edge reachable from outside the
machine; a gateway that keeps a connection across an edge's death; Raft; several
stores; object storage; the autoscaler, the operator, a Helm chart (M4); splitting
or slowing a region for load (M5); moving players between edges for balance; chat;
an inventory beyond the hotbar; mutual TLS; replacing what a lost node had, which
is an operator's or M4's.

## 10. What is still not known

- Whether the 26.3 client follows a transfer in play without a prompt, falls when
  placed in the air without the flying bit, and how long it waits on a silent
  server. Whether the official server's `/transfer` can be driven from a test.
- Everything about Kubernetes and Linux marked as a guess: how long a dead node
  goes unnoticed, what a Service, a headless name and a port-forward do meanwhile,
  when pods are replaced, kind's volumes and its NetworkPolicies, the lease
  client's defaults, how long a connect lasts. R0.6 tries what it can.
- How long a standby store takes to be promoted and what a mirrored sync costs.
  Commit latency on a real disk was never timed (roadmap, line 296).
- The edge's resume logic under section 4.1: an entering stay, a dead stay and a
  floor through a link that is lost, a merge and a split. Not traced; it is what
  ADR-0020's reviews are for.
- Where in `lanes.rs` the records and floors are kept once a segment is removed. I
  read that a file of the store's own is needed, not how recovery reads it.
- I read `fanout.rs` and the runner only in the parts cited, and ADR-0011, 0012,
  0014, 0016 and 0017 by section.

## 11. What the review changed

Each finding was checked against the code it cites. All fourteen hold in what they
say is true. Three of their proposed remedies are not taken, with the reason.

| # | Finding | | Why |
|---|---|---|---|
| 1 | Asking every region once does not make one stay per player | **Accepted**; its remedy in part | (a) holds: a table routes only regions with an owner (`crates/clustine-region/src/lib.rs:29-43`). (b) holds: an arrival is kept and sent again (`fanout.rs:2682-2700`), and a replaced stay's edge is told nothing (`region.rs:694-702`). (c), (d) hold. The supersede is withdrawn. Taken from the remedy: order by the stay's number, and every replacement tells the loser's edge. **Not taken**: "an edge known dead in seconds, so that regions drop its players". It leaves a join unable to know whether the old stay's record is written yet, and an edge cut off from the coordinator would lose every player. The store's floor does not depend on anybody noticing a death (4.1) |
| 2 | Nothing orders two records; a stay that ends on the way writes none | **Accepted**; one remedy not needed | The record names its stay and hops and the store keeps the highest. **Not taken**: `Discard` and `drop_edge` writing a record (`region.rs:512-517, 632-691`). The record is written in the tick that lets a player go, which is durable before anybody acts on it (`region.rs:560-571`, ADR-0008); a stay that ends on the way has its place already. `Discard` gets the hops, for finding 11 |
| 3 | A player who was flying comes back falling | **Accepted** | `fanout.rs:52` has no flying bit and nothing reads the client's. In R1 (4.1, R1.2, R1.4, R1.6) |
| 4 | A join at a kept place makes the home region claim land | **Accepted**, both parts | The edge asks the home region for the view of whoever it places (`fanout.rs:2074-2126`). Rule 6: the home region lets the stay go without placing it. The answer reaches a tick as an input, so nothing waits behind a join (`sim/state.rs:60-61`). The case nobody holds is measured in R1.7, with what follows |
| 5 | Silence: more than one crate, connects without an end, deadlines that follow the lease | **Accepted** | `client.rs:664` and `crates/clustine-rpc/src/tcp.rs:55` have no deadline; the store's sockets are its own. R0.4 is three steps; section 5 has the deadlines |
| 6 | The worst case is never added up; one fence costs a leader | **Accepted** | Section 5. "Closes every connection" became "stops deciding"; the holder is on disk; the same holder goes on without a grace (4.3). Taken differently: the part on the lease is in ADR-0024, reviewed before R3.1, so ADR-0026 need not be written first |
| 7 | The term: fences incomplete as worded | **Accepted**, all of it | Fence 2 is about a hello that raises an epoch; the refusal says the term; the places are four (`state.rs:1514` was missing); the store's answers carry the term; a restored store goes above what it is shown; the elector is behind an interface; `alone` asks for a term |
| 8 | Builds that cannot read each other; a build with another state format drops players without a record | **Accepted**; one remedy not needed | A wire number in one greeting (4.6); which updates are rolling (section 3). **Not taken**: the edge's drain in R1 so that an orderly stop writes records. Records no longer depend on how a stay ends. New from it: the entity id must survive a dropped state (4.1, rule 9) |
| 9 | Weights in place of counts move nothing; the claim about parts is the wrong way round | **Accepted** | Count as the floor, tick time on top; the trigger and the choice as the review has them; the claim is withdrawn and is question 9 |
| 10 | No real client reaches a second edge on Kubernetes; much rests on Kubernetes noticing | **Accepted** | Questions 7 and 8; the roster carries public addresses and who drains; edges surge; "one node" said in section 3; the store's node not drained before R5 |
| 11 | A version orders introductions, not removals or absence | **Accepted** | The table in 4.2; three tests |
| 12 | What the old stay did to another region's blocks outlives it | **Accepted** | Actions and their "done" name the stay (4.1, rule 8); "in doubt" lasts until the old edge is gone (section 7) |
| 13 | No sizes; steps out of order; R1 tested with one edge; nobody has the single process; greetings change three times | **Accepted** | Sizes on every step; the transfer packet and the bots that follow it are both in R2.5; R1.7 has two edges; R1.5 and R3.4 name the single process; one greeting in R0.3; the critical path said |
| 14 | The standby: what the list of attacks lacks | **Accepted** | Added to 4.5 |
