# N7h: a player who logs out on a mount takes it with them

Design of 2026-10-21, revised after two independent reviews (section 12) into one text:
sections 3 to 10 are what is built. It answers section 10 item 1 of
[the riding plan](n7h-riding-plan.md): the game's taking of a lone rider's vehicle out of
the world at logout, for every root vehicle, against ADR-0020's stays and ADR-0028's
arrivals. Until it is built a player who leaves riding is put off beside the vehicle
(`riders.rs`, `unseat_leaving`; ADR-0020's N5 amendment).

## 1. What the game does, and what a player sees

`PlayerList.remove` (R8 of [the research](n7h-riding-research.md)): if the root vehicle
`hasExactlyOnePlayerPassenger()`, `saveParentVehicle` writes the root and all its
passengers into the player's data and removes each as `UNLOADED_WITH_PLAYER`; with two
players aboard it stays. At login the unit is added again with its UUIDs and the player
seated. The probe `probe_a_rider_who_leaves_on_a_pig_on_the_official_server`
(`bin/clustine/tests/mobs_oracle.rs`, 2026-10-21) confirmed it for 26.3: a watcher sees
no pig while its rider is away, and the rider comes back seated on it.

After this step: leave on a pig, a cart, or a boat with a pig behind you, and it goes
with you; come back and you are on it, where it was. A boat with two players stays.
**Named**: where the leave and the next login race (the edge crashed, a login through
another edge, the leave's region hung or lagging) the player comes back beside the
vehicle and not on it; the vehicle is never lost or doubled.

## 2. The options

**A. Parked in its own chunk**, the place naming it. Rejected: park and reference are two
durable facts, and the store may drop the note that carries the reference. In the quick
reconnect (the new stay's `Entering` durable before the leave's commit) the park counts
and the reference is dropped: an invisible unit for the life of the world, which no
region can reclaim, since none keeps floors (ADR-0020, departure 3). And every reader of
entities (the tick, collisions, copies, the edge, spawning's counts, hoppers, pick-ups,
sculk, sensing, leashes) would have to skip a parked one.

**B. The store keeps the unit with the player's record, and turns a note it cannot take
into an arrival back into the unit's chunk.** Chosen. The commit thread already orders
every note against every floor (ADR-0020, section 3); it decides once, atomically with
the commit that takes the unit out of its chunk, where the unit lives. Every durable
state has the unit in exactly one of: a chunk, a filed arrival, a record. Off the world
it is where the game has it; on the way back an ordinary arrival (ADR-0028, section 5)
and the player an ordinary rider whose vehicle is pending (N5).

**C. Park, wait for the store's word, then remove**: both hold it between the commits,
and an `Enter` then doubles it. **D. The edge keeps it**: an edge is not durable.

## 3. Leaving with the unit (the sim)

**Who takes it.** `remove_player` only: a leave that names the stay (`apply_leave`) and
an edge's reset or going (`drop_edge`). There, if the player is seated (not pending) in
a unit of the region's whose seats, over every member, hold exactly one player, seated
or pending, the tick's commit has:

- every member `Gone` from the root's chunk, reported `EntityGone` at its `hops` (h), a
  removal and not "handed on";
- the removal note `Has { place, unit: Some(the members, root first, as ADR-0028's unit
  arrival) }`, `place` being the dismount pose the vehicle would let them off at (as
  `unseat_leaving` works it out now), so that wherever the unit is not put under them
  again they stand beside it. The unit's seat for the player stays as it is: seated,
  naming the leaving stay's key;
- then `Gone`, as now.

Otherwise (two players, a pending rider, a pending leaver) the N5 amendment stands: noted
at the dismount pose, the unit left. **`end_player`** (`apply_dead`, a join or an arrival
over the stay) takes no unit and keeps its dismount note through `unseat_leaving`: the
region knows the stay dead, its note would be dropped, and the unit would only come back
as an arrival. Named: a second login while the first rides does not ride.

**The notes' map** (`Region::notes`, keyed `(player, rank, entity)`). A removal note is
keyed by **its own stay's entity at rank 0**, so that nothing of the same tick replaces
it: not a let-go `Has` (rank 2, which today shares the key `(player, 1, EntityId(0))`
with every `Has` of the player), not another stay's removal. `Entering` is rank 1, the
present or let-go `Has` rank 2, `Gone` rank 3 keyed by its entity. So a removal comes
before an `Entering` of the same player, and a reconnect whose leave and join one region
applies in one tick keeps the ride (section 4: the removal puts the unit in the record,
the `Entering` then finds it). Either order is safe for ADR-0020's table.

**Take-ins drop a stale seat.** A seat that is a player's, not pending, and whose player
is not in the region riding that entity is dropped at every take-in. `take_up_entities`
(`entities.rs`) does so at a load; `take_arrivals` (`arrivals.rs`) keeps such seats
today, and is given the same rule. So a unit sent back to its chunk arrives empty.

## 4. The store's side

**The record** gains `unit: Option<Unit>` (`Unit`: the members as
`clustine_world::Entity` values). **`Has`** gains `unit: Option<Unit>`. **A commit**
gains `units_taken: Vec<(PlayerId, EntityId)>`, the home region's takings (section 5),
written outside the notes' map.

**The trial.** When the commit thread takes a commit, after the owner's test, **before
the commit's land nobody holds (`nobodys`) is worked out and before it is appended**, it
runs a pure trial of all the commit's notes in their order against the records as the
group's earlier commits leave them, each note applied in turn as `load` applies notes in
log order. A unit's fate:

| The note in the trial | The record's unit | The store |
|---|---|---|
| Counts (`Written`) | None | Keeps the unit in the note; on apply, the record takes it |
| Counts | Some (cannot be: a stay in the world has had its unit taken) | Sends the note's unit back, as below; logs an error |
| `Dead`, a copy, `Ahead` | – | Sends the unit back |

**Sending back**: the unit leaves the note and becomes one of the commit's arrivals,
indexed after the region's own, addressed to the root's chunk, at `hops` h, filed by
`Table::holder` (normally the sender, which takes it back), land nobody holds granted to
the sender in the group, its `Granted` record ahead of the commit's since the trial comes
before `nobodys` (ADR-0028, section 5).

**The log has the commit as transformed.** `apply` and recovery judge nothing about
units: a note's unit goes into the record and a taking clears it, as logged. Replay
equals the running store (the first review checked `Log::fail`, `settle`, `make_over`).

**A taking** `(P, K)` clears `record.unit` of P if its root is K, with no stay test: the
commit that carries it has the unit in a chunk or an arrival, and that commit counts. A
taking of another key changes nothing. A failed group takes the record's unit back with
the record (`Undo`).

**Takings count as notes everywhere notes are counted**, or a restart would replay a
unit into a record without the taking that cleared it: the commit arm's early return
(`notes.is_empty() && gone.is_empty() && !names_all` in `lanes.rs`), `players.noted` (the
segment kept for the players), `load`'s `noted` (pushed where `!stays.is_empty()`),
`Opened`'s `retain` of what a region's commits above `restored` said, and segment
keeping (`trim_for_the_players`, rule P).

**`Enter`** gains `unit: Option<Unit>`, the record's (`Raised` and `Again` alike), and
`holder` is the holder of the **root's chunk** when there is a unit, a rider's holding
chunk being their root's (ADR-0028, section 5). It is answered to the home region in its
own group, as now; sending a unit back answers nobody.

**Size.** A unit in a note is written with the arrivals' entity codec and counted by the
record split rules as an arrival is; a commit whose content is notes and the unit's
`Gone` changes takes the split path, not a notes-only shortcut.

## 5. Re-entry (the home region, step 7)

With `answer.unit = Some(u)`, the home region H, if it still holds the stay N as
entering (otherwise the answer is passed over and the unit stays in the record for the
next login), in this tick's commit:

1. Puts `(P, root)` into `units_taken`.
2. Rewrites `u`'s player seat to N's key.
3. Puts the unit back:
   - **made whole in H** only where it would stay there: the predicate `send_entity`
     decides by (the root's chunk H's own and loaded, not a copy, not lent, lending or
     being returned). Every member at `hops` h + 1 (a take-in, ADR-0028 section 6), the
     seat seated, N seated at once;
   - **anywhere else, as an arrival** to the root's chunk at `hops` h (its taker raises
     to h + 1), the seat pending with N's edge and `PENDING_FOR` ticks, N's riding
     `PlayerRiding { vehicle: root, chunk: root's chunk, pending: Some(PENDING_FOR) }`.
4. Places or lets N go as step 7 does, at the unit's seat pose:
   - `holder` none or H: N placed in H, holding chunk the root's, which H claims if it
     knows nothing of it. Land nobody holds is also granted to H with the arrival: the
     claim is answered twice, harmlessly, as the worker keeps arrivals by number. H loads
     the chunk with the arrival ticket, takes the unit and seats N;
   - `holder` X: N let go to X unplaced (`Departed`, hops 1, the attempt) **in the same
     commit as the unit's arrival**: N5's crossing of a unit with a rider, whose halves
     meet in X in either order;
   - **the place below the world**: N at the spawn point, not riding; the unit back to
     its chunk as an arrival with its seat dropped.

`holder` is advice. If the arrival is filed elsewhere than N went, N's holding chunk is
the root's, and N5's departures hand N on to its holder. `PENDING_FOR`, 100 ticks, is
counted as an age by the region that holds each half, so a region that is away counts
from when it runs. A seat or rider not met in that time is dropped by N5's rules: the
unit stays, empty; N stands at the seat pose. Until the unit is introduced the client is
in the air at the seat pose and its moves are kept (N5): no wait in the common case;
after a long one the seating ends with a teleport (named).

**Keys and hops.** The unit keeps its keys and UUIDs, so references among members hold.
It returns at `hops` h + 1, above its leave's `EntityGone`: an edge that keeps the
removed key (`was_gone`: `hops <= gone.hops`) takes it as a new introduction
(`upsert_entity`), one that forgot it after 600 ticks likewise, and a late snapshot at h
is passed over. Its counters are its own `lived`, still in the record. A unit sent back
blinks for watchers (gone, then introduced again): named.

**Leashes.** Among members they hold by key. A leash from a member to a knot or to an
entity outside the unit is kept in the unit and broken at the outside end as the game
does at `UNLOADED_WITH_PLAYER`, which a probe confirms before it is built.

## 6. The edge: the riding word with the entity

The edge takes a riding word only for a view with an entity (`fanout.rs`,
`PlayerEvent::Rides`) and from a transfer (`hand_over`, path 3). A rider entered by
`Spawned`, or by `Present` after a lost link (ADR-0020, section 4.3, path 1), would lose
it, and their own client would not know it rides. So:

- `PlayerEvent::Spawned` and `Presence::Present` gain `riding: Option<EntityId>`, the
  vehicle of the player's state, seated or pending;
- `spawn_player` takes it (`take_rides_word`) after noting the entity and before the
  client's packets, as `hand_over` takes a transfer's first; a `Present` for a view with
  the entity takes it again, so a resume mends a word lost with a link;
- a `Rides` for a view without an entity stays passed over.

`effective_passengers` then lists N in the root's seats from the word alone, so every
client, the rider's own among them, is sent `set_passengers` when the unit is introduced,
whichever comes first.

## 7. Merges, splits, hand-overs, restores

There is no parked unit. One in a record is untouched by any change of regions; one on
its way back is an arrival, which moves with its chunk's holding (ADR-0028, section 5),
its pending seat kept at a take-in while it has time left and dropped when its edge goes
(N5, section 8 items 1 and 9). An `Enter` dropped by a merge or a split is said again,
unit and all. A home region restored before its taking commit was durable gets `Again`
with the unit; one restored after has N placed and no `Enter` to read.

## 8. Every order of events

D: the leaving stay, in X, on unit U. N: the next stay. H: the home region.

| # | Order | Outcome |
|---|---|---|
| 1 | Leave's commit taken before N's `Entering` | U in the record; N's `Enter` carries it; N comes back on U |
| 1a | Leave and join applied by one region in one tick | Removal note before `Entering`: as 1 |
| 2 | N's `Entering` durable first (an edge crashed, another edge, X hung or lagging) | D's note dead; U sent back; X takes it, D's seat dropped; N at the last place the store had, beside U, not riding |
| 3 | X taken over, the old owner's leave commit taken | As 1 or 2 |
| 4 | X taken over, that commit dropped as a replaced owner's | The new owner restores D on U, gets the kept leave again; as 1 or 2 |
| 5 | D replaced (`Dead`, a join, an arrival) | Put off; U stays; N not riding |
| 6 | D's edge reset or gone | As a leave |
| 7 | Two stays of P in one tick (a removal and a let-go, or two removals) | Each note under its own entity; none replaced |
| 8 | N leaves while entering, or N2 replaces N before H reads `Enter` | U stays in the record; N2's `Enter` carries it |
| 9 | H took U; N2 joins before N is seated | U arrives with N's pending seat, which lapses; U empty; N2 not riding |
| 10 | X crashes before the leave's commit is durable | As 4 |
| 11 | The store crashes anywhere | Recovery reads the transformed commits and the takings |
| 12 | The store restarts with a segment whose only stay entry is a taking | The taking is replayed; U not doubled |
| 13 | H crashes after `Enter`, before / after its commit | `Again` with U / U in a chunk or filed, the record cleared |
| 14 | A group with any of these fails | `Undo` takes back the record's unit, the taking and the arrival together |
| 15 | Place below the world | N at the spawn; U back to its chunk, empty |
| 16 | Place in X's land; X away | N kept at the edge for X, U filed for X; both taken when X runs |
| 17 | Rider's chunk and root's chunk differ | `holder` and the holding chunk are the root's |
| 18 | Two players aboard, or a pending second rider | The leaver put off; U stays |
| 19 | N's link to H lost before `Spawned` | `Present` enters N with the riding word |

## 9. What ADR-0020, ADR-0028 and the format need

**ADR-0020**, "Amended with step N7h": the record's `unit` (section 2); `Has`'s `unit`,
the commit's `units_taken`, section 4's table, the trial and the transform (section 3);
the notes' ranks; `Enter`'s `unit` and the root's holder; step 7 as section 5; the riding
word in sections 4.1 and 4.3; section 8's rows in section 10; the N5 amendment narrowed
to `end_player`'s paths and section 3's "otherwise". **ADR-0028**: the store can make one
of a commit's arrivals from its note (section 5); a take-in of an arrival drops stale
seats as a load does; its N5 amendment's named difference goes.

**The format.** In a commit's stays: a note kind 3, `Has` with a unit (the unit after the
place, by the versioned entity codec of chunks and arrivals), and an entry kind 4, a
taking (u128 player, u64 root key), which is no stay note; kinds 1 and 2 read as now, so
an older log reads unchanged. The players' file's version is raised: a record gains a
byte for a unit and the unit after the place, by the same codec; a file written before
is read without units. `LoggedPlace` stays `Copy`; the store's `Record` and
`PlayerRecord` become `Clone`, and `Taken::Raised`/`Again` carry the unit.
`StoreReply::Enter`, `Entered`, `TickOutput`, `Spawned` and `Present` change, so the
wire number is raised. No `STATE_FORMAT` change.

## 10. Tests, by someone who did not build it

**The invariant**, asserted after every step: each member key is in exactly one of a
chunk's entities, a filed arrival or a record, read from a stopped store.

- **Store**: section 4's table, a row a test; the trial with two notes of one player in
  one commit and with an earlier commit of the group, by `load`'s semantics; the sent-back
  arrival's index and its `Granted` ahead of the commit; a taking with a dead note
  clearing the unit, one of another key changing nothing; `Enter` and `Again` with the
  unit and the root's holder; every path into `fail_log`; **each place of section 4's
  "count as notes" by a test that fails without it** (a commit with only a taking, a
  restart replaying it, an `Opened` that passes over it, a segment kept for it); kills
  between the leave's commit and `Entering`, between `Entering` and its answer, and while
  the players' file is written; recovery equal to the running store; a chest boat unit
  across the split rules; the older players' file read.
- **Sim**: each condition of section 3 (two riders, a pending second rider, a pending
  leaver, a pig in a boat, a cart); `end_player`'s paths; **two stays of one player in
  one tick, each note kept**; a leave and a join in one tick, removal before `Entering`;
  **an arrival with a seated seat of an absent player arrives empty**; step 7 in each
  case of section 5 (whole in H by `send_entity`'s predicate, an arrival otherwise, below
  the world), the seat rewritten, the taking, hops h + 1; a restore after the taking.
- **Runner**: `Enter` with a unit dropped by a merge and said again.
- **Edge**, against scripted regions: `Spawned` and `Present` with `riding` before and
  after the unit is shown; a `Rides` for a view without an entity, then `Present`;
  `set_passengers` to the rider's own client; a unit gone at h and introduced at h + 1
  within and after 600 ticks; a late report at h passed over.
- **End to end**, every row of section 8 by name, in one process and a cluster: *a
  quick reconnect whose `Entering` reaches the store before the leave's commit leaves the
  pig in its chunk* (X frozen between leave and commit); *a leave from a region being
  taken over*, rows 3 and 4; each process killed at each step; a pinned line between
  place and home; land nobody holds, with the claim and the grant in both orders.
- **Against the official server**: the probe above; one for leashes at
  `UNLOADED_WITH_PLAYER`; what a client is sent at a login on a pig, in order.

## 11. Open questions

1. **Where a raced reconnect stands.** Row 2 puts N at the store's last place, which
   while riding is a seat pose, so N stands on or by U as it comes back. Writing the
   dismount pose in a rider's every `Has` would put them beside it; worth the bytes?
2. **Row 9** (a second login before the first is seated) leaves U empty; named, rare.
3. **The leash probe** may change section 5's last paragraph.

## 12. The reviews, and how each finding was settled

**First review** (2026-10-21): the choice right; four ways to double the unit or lose
the ride.

1. A taking inside a place note could be replaced in the notes' map: `units_taken`,
   outside it (section 4).
2. A unit made whole where it would not stay: only by `send_entity`'s predicate
   (section 5).
3. A same-tick reconnect lost the ride: removal notes before `Entering` (section 3).
4. A late unit for raced reconnects: **withdrawn** after the second review (below).
5. The decision: a pure trial before `nobodys`, the arrival after the region's own
   (section 4).
6. Recovery does not judge (section 4).
7. The leaver's seat written seated, dropped at the take-in; the place the dismount
   pose; the blink named (sections 3, 5).
8. The client in the air at the seat pose until the unit comes; named (section 5).
9. The players' file by the versioned entity codec (section 9).

Its answers to the first draft's open questions: the game does put the unit back (the
probe); below the world, back to its chunk empty; the claim answered twice harmlessly;
100 ticks suffice; leashes as section 5; a mid-crossing leaver leaves the unit behind
(named); recovery's disagreement path gone.

**Second review**: the late unit still doubled or lost the unit, and the text
contradicted itself.

1. A late unit taken twice, from `Unit` and from `Enter`; 3. `end_player` both taking
   and not taking a unit; 4. the late unit's region side underspecified; 5. resending
   `Unit` only at a first tick: **settled by dropping the late unit** (the coordinator's
   decision). A note that does not count sends its unit back to its chunk; a raced
   reconnect comes back beside the vehicle, named, never a loss.
2. A removal note overwritten in the notes' map: keyed by its own entity at rank 0
   (section 3), tested with two stays of one player in one tick.
6. A leaver's seat surviving an arrival: `take_arrivals` drops stale seats as
   `take_up_entities` does (section 3).
7. Takings not counted as notes would double the unit at a restart: counted in each place
   named (section 4), each tested.
8. Stale text (`took` on `Has`, a taken key in the note, row 2): this rewrite.
9. `Enter` answered in H's own group, nothing into another owner's queue; the trial by
   `load`'s sequential semantics and the arrival index; a large unit by the split rules
   (section 4).

## 13. The third review (2026-10-21), and what the design is now

A third independent review followed every order of section 8 through `region.rs`,
`riders.rs`, `arrivals.rs` and `lanes.rs` and found no path that doubles or loses a mob,
and the two earlier reviews' findings settled in the text but for one. It found these, and
the design stands amended by them:

1. **A stop, or an edge's restart, then a quick rejoin** (sections 1, 3 and row 6). Only
   `remove_player` takes the unit, and `Server::stop` ends the edge first, so no leave is
   sent: a player who stops the server on a pig and joins again within the 600 ticks of
   `EdgeEvent::Gone` stands beside an empty pig, and after them is put on it. The owner
   would meet that within minutes. **A graceful stop now ends every view as a leave** (the
   game's `PlayerList.removeAll`, which goes through `remove`) and waits until the regions
   have committed those leaves before they stop; the same serves an edge pod's SIGTERM. A
   crash of an edge or of the whole process stays as it is, named. A row and an
   end-to-end test: stop while riding, start, join at once, seated.
2. **Every dismount note is keyed at rank 0 by `(entity, hops)`** (section 3), the end's
   as well as the removal's, and inserting a note that would replace one carrying a unit
   is an invariant broken (`expect`). Today `end_player`'s note shares `(P, 1, 0)` with the
   present `Has`, which it replaces when `apply_arrival` ends an earlier stay: the
   arrival's note, hops and face are lost for that tick. That is a defect of now, mended
   with this step and tested.
3. **A commit's takings are applied before its notes**, in the trial, in `apply` and in
   replay alike (section 4); a test holds a taking and an `Entering` of the same player in
   one commit. Today it holds only because `Region::tick` applies the edge's events and
   the dead before the entered.
4. **Where a re-entry's wait lapses, the player is put at the record's place, the
   dismount pose** (section 5), not at the seat pose where they wait, which is in the air
   or in water. After a logout of minutes the root's chunk is seldom loaded in the home
   region, so the arrival with the player pending is the usual path, not the rare one; its
   wait is a chunk's load, behind the client's loading screen. The lapse is tested.
5. **The probes keep a boat alone, two players in a boat, and add a minecart** (section
   10): only the official server checks these packets. Named in section 1: the game saves
   a rider's place as (the vehicle's x, their own y, the vehicle's z), so a player who
   leaves a boat with another player in it comes back at the boat's centre in the game,
   and at the dismount pose here (N5's, as now).
6. Accuracy: a unit goes as one arrival per member, the root first, in `send_entity`'s
   order, and the stale-seat rule is applied member by member; section 9's list of changed
   messages also names `StoreRequest::Commit` (it gains `units_taken`) and `StayNote::Has`.

## 14. What the building found (2026-10-21)

1. **The riding word is taken after the login**, not before the client's first packets
   (section 6): the word's `set_passengers` can reach the rider's own client, which must
   be sent its login first. It is taken after the login and before the view, so the unit
   is still introduced to them, and to everyone with them, with them in its seats.
2. **A leaving rider's unit chunk is held through the tick** (section 3): a rider's view
   goes with their leave, and with nobody else watching, the release of their tickets at
   the start of the tick let the chunk go before the leave was applied, and the unit was
   not there to take. The chunk of every seated player who leaves in the tick (a leave,
   an edge's reset or going) is held by an arrival's ticket that ends with the tick, and
   let go at its end where nothing else keeps it.
3. **The re-entry always sends the unit back as an arrival**, never makes it whole in the
   home region at once (section 5, first case): the review found the arrival the usual
   path, and one path for every case leaves fewer orders to get wrong. The cost is the
   arrival's round through the store, a tick or two.
4. **Leashes from a member to beyond the unit** are kept in the unit as they are; the
   probe of `UNLOADED_WITH_PLAYER` (section 5's last paragraph) is still to come.
