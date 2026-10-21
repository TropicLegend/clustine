# N7h: a player who logs out on a mount takes it with them (draft)

Draft of 2026-10-21, for an independent review before anything is built. It answers
section 10 item 1 of [the riding plan](n7h-riding-plan.md): the game's taking of a lone
rider's vehicle out of the world at logout, for every root vehicle, designed against
ADR-0020's stays and ADR-0028's arrivals. Until it is built a player who leaves riding is
put off beside the vehicle (`riders.rs`, `unseat_leaving`; ADR-0020's N5 amendment).

## 1. What the game does, and what a player sees

`PlayerList.remove` (R8 of [the research](n7h-riding-research.md)): if the player's root
vehicle `hasExactlyOnePlayerPassenger()`, `saveParentVehicle` writes the root with all
its passengers into the player's data, and every entity of `getPassengersAndSelf()` is
removed as `UNLOADED_WITH_PLAYER`; with two players aboard it stays. At login
`loadAndSpawnParentVehicle` adds the unit again with its UUIDs and seats the player
(`startRiding(…, true, false)`). Boats, carts and mobs alike; meanwhile the unit is in
no chunk.

After this step: leave on a pig, a cart, or a boat with a pig behind you, and it goes
with you; come back and you are on it, where it was. A boat with two players stays.

## 2. The options

**A. Parked in its own chunk** (the review's first proposal). The leave's commit marks
the unit parked (a `Set` in its chunk) and the place names it; the entering region
unparks it.

- For: the store learns nothing about entities.
- Against, decisive: park and reference are two durable facts, and the store may drop
  the note that carries the reference. In the quick reconnect (X lags, the new stay's
  `Entering` is durable before X applies the leave) the park counts and the reference
  is dropped: an invisible unit for the life of the world. No region keeps floors
  (ADR-0020, departure 3), so none can tell whether anyone will come for a park.
- Against: every reader of entities must skip a parked one (the tick, collisions,
  copies, the edge, spawning's counts, hoppers, a cart's pick-up, sculk, sensing,
  leashes). Each one missed is a seam.

**B. The store keeps the unit with the player's record, and turns a note it cannot take
into an arrival back into its chunk** (the review's second proposal, with the arrival
decided before the commit is appended).

- For: the store's commit thread already orders every note against every floor
  (ADR-0020, section 3). Whether the leave's place counts, and so where the unit goes,
  is decided there, once, in the commit that takes the unit out of its chunk. Every
  durable state has the unit in exactly one of: a chunk, a filed arrival, a record.
- For: there is no parked state. Off the world the unit is where the game has it, in
  the player's data; on the way back it is an ordinary arrival (ADR-0028, section 5) and
  the player an ordinary rider whose vehicle is pending (N5).
- Against: the store's record and `Enter` carry entities, and the store edits a commit
  before appending it.

**C. Park, wait for the store's word, then remove**: between the two commits both the
record and the chunk hold the unit, and an `Enter` then duplicates it. **D. The edge
keeps it**: an edge is not durable.

**Chosen: B**: one thread decides where the unit lives, atomically with the commit that
takes it out of its chunk; A needs a reclamation rule nobody can write.

## 3. Leaving with the unit (the sim)

`unseat_leaving(Some(id), …)` is reached from `remove_player` only: a leave that names
the stay (`apply_leave`) and an edge's reset or going (`drop_edge`). There, if the
player is seated (not pending) in a unit of the region's whose seats, over every
member, hold exactly one player, seated or pending, then that tick's commit has every
member `Gone` from the root's chunk, reported `EntityGone` at its `hops` (h), a removal
and not "handed on"; and the stay's note `Has { place: the seat's pose, unit: Some(the
members, root first, as ADR-0028's unit arrival) }`, then `Gone` as now. Otherwise the
N5 amendment stands: noted at the dismount location, the unit left.

**Not from `end_player`** (`apply_dead`, a join or an arrival over the stay): the region
knows the stay dead, its note would be dropped and the unit come straight back, a
flicker for nothing. Named: a second login while the first rides does not ride.

## 4. The store's side

**The record** gains `unit: Option<Unit>` (`Unit`: the members as `clustine_world::Entity`
values). **A `Has` note** gains `unit: Option<Unit>` and `took: Option<EntityId>`.

When the commit thread takes a commit, after the owner's test and **before appending
it**, it works out each unit note's fate by ADR-0020's section 3 table, which `apply`
will then find, as nothing runs between:

| The note | The record's unit | The store |
|---|---|---|
| Counts (`Written`) | None | Keeps the unit in the note; on apply, `record.unit = unit` |
| Counts | Some (cannot be: a stay in the world has had its unit taken, below) | Moves the note's unit into the commit's arrivals; logs an error |
| `Dead`, copy or `Ahead` | – | **Moves the unit out of the note into the commit's arrivals**, last in their order, addressed to the root's chunk, with its `hops` |

The moved unit is then an arrival of the commit like any the region sent: numbered,
filed by `Table::holder` (normally the sender, which takes it back), land nobody holds
granted to the sender in the group (ADR-0028, section 5). **The log has the commit as
transformed**, so recovery reads where the unit went and does not decide again; where
its replay of a place disagrees, the logged form wins and an error is logged (open
question 8).

A `took: Some(K)` note, which only the home region writes (section 5), clears
`record.unit` if its root is K, **whether or not the note's place counts**: the commit
that carries it has the unit in a chunk or an arrival, and that commit counts. A group
that fails takes `record.unit` back with the rest of the record (`Undo`).

**`Enter`** gains `unit: Option<Unit>`, the record's (`Raised` and `Again` alike), and
`holder` is the holder of the **root's chunk** when there is a unit, since a rider's
holding chunk is their root's (ADR-0028, section 5).

## 5. Re-entry (the home region, step 7)

With `answer.unit = Some(u)`, the home region H, if it still holds the stay N as
entering (otherwise the answer is passed over and the unit stays in the record for the
next login):

1. Rewrites `u`'s one player seat to N's key, pending, with N's edge and
   `PENDING_FOR` ticks (N5's `Pending`).
2. Puts the unit back, **in this tick's commit**, and writes `took: Some(root)` on N's
   note:
   - root's chunk loaded in H: `Whole` there, every member's `hops` h + 1 (a take-in,
     ADR-0028 section 6), N seated at once (`pending: None` below);
   - otherwise: as an arrival addressed to the root's chunk, with `hops` h, which its
     taker raises to h + 1.
3. Places or lets go N as step 7 does, with `riding: Some(PlayerRiding { vehicle: root,
   chunk: root's chunk, pending: Some(PENDING_FOR), .. })`:
   - `holder` None or H: N is placed in H at the place, seated with the vehicle
     pending, their holding chunk the root's, which H claims if it knows nothing of it.
     Land nobody holds is granted to H with the arrival in the same group; H loads it
     with the arrival ticket, takes the unit, seats N (open question 3);
   - `holder` X: N is let go to X unplaced (`Departed`, hops 1, the attempt) **in the
     same commit as the unit's arrival**: N5's crossing of a unit with a rider, whose
     two halves meet in X in either order;
   - **the place is below the world**: N goes to the spawn point as now, not riding;
     the unit goes back to its own chunk as an arrival with no player seat (open
     question 2).

`holder` is advice. If the arrival is filed elsewhere than N went, N's holding chunk is
the root's, and the ordinary rules hand N on to its holder (N5's departures). A seat or
rider not met within `PENDING_FOR` is dropped by N5's rules: the unit stays, empty, the
player at the place. Named.

**Keys and hops.** The unit keeps its keys and UUIDs, as the game its UUIDs, so
references among members hold. It returns at `hops` h + 1 or more, above its leave's
`EntityGone`: an edge that keeps the removed key (`was_gone`: `hops <= gone.hops`)
takes it as a new introduction (`upsert_entity`), one that forgot it after 600 ticks
likewise, and a late snapshot at h is passed over. Its counters are its own `lived`,
which stands still in the record.

## 6. The edge: the riding word with the entity

The edge takes a riding word only for a view with an entity (`fanout.rs`,
`PlayerEvent::Rides`) and from a transfer (`hand_over`, path 3). A rider entered by
`Spawned`, or by `Present` after a lost link (ADR-0020, section 4.3, path 1), would have
it dropped, and their own client would not know it rides. So:

- `PlayerEvent::Spawned` and `Presence::Present` gain `riding: Option<EntityId>`, the
  vehicle of the player's state (seated or pending);
- `spawn_player` takes it (`take_rides_word`) **after noting the entity and before the
  client's packets**, as `hand_over` takes a transfer's before anything else; a
  `Present` for a view that has the entity takes it again, so that a resume corrects a
  word lost with a link;
- a `Rides` word for a view without an entity stays passed over: the `Spawned` or
  `Present` that enters it carries the current word.

`effective_passengers` then lists N in the root's seats from the word alone, so every
client, the rider's own among them, is sent `set_passengers` when the unit is introduced,
whichever of the player and the unit it hears of first.

## 7. Merges, splits, hand-overs, restores

There is no parked unit. One in a record is untouched by any change of regions; one on
its way back is an arrival, which moves with its chunk's holding (ADR-0028, section 5),
its pending seat kept at a take-in while it has time left and dropped when its edge goes
(N5, section 8 items 1 and 9). An `Enter` dropped by a merge or a split is said again,
unit and all. A home region restored before its `took` commit was durable gets `Again`
with the unit; one restored after has N placed and no `Enter` to read.

## 8. Every order of events

D: the leaving stay, in X, on unit U. N: the next stay. H: the home region.

| # | Order | Outcome |
|---|---|---|
| 1 | Leave's commit taken before N's `Entering` | U in the record; N's `Enter` carries it; N comes back on U |
| 2 | N's `Entering` durable first (X lagged, hung, or frozen) | D's note dropped; U moved into the commit's arrivals; X takes it back, D's seat dropped at the take-in (N5, section 8 item 1); N at the last counted place, not riding |
| 3 | X taken over, the old owner's leave commit taken | As 1 or 2 |
| 4 | X taken over, that commit dropped as a replaced owner's | The new owner restores D on U and gets the kept leave again; as 1 or 2 |
| 5 | D replaced (`Dead`, a join, an arrival) | Put off as now; U stays; N not riding |
| 6 | D's edge reset or gone | As a leave: U into the record or back as an arrival |
| 7 | N leaves while entering, or is replaced by N2 before H reads `Enter` | U stays in the record; N2's `Enter` carries it |
| 8 | H read `Enter` and took U; N2 joins before N is seated | U arrives with N's pending seat, which lapses; U stays empty; N2 not riding |
| 9 | X crashes before the leave's commit is durable | As 4 |
| 10 | The store crashes anywhere | Recovery reads the transformed commits |
| 11 | H crashes after `Enter`, before / after its commit | `Again` with U / U in a chunk or filed, the record cleared |
| 12 | A group with any of these fails | `Undo` takes back the record's unit, `took` and the arrival together |
| 13 | Place below the world | N at the spawn; U back to its chunk, empty |
| 14 | Place in X's land; X away | N kept at the edge for X and U filed for X; both taken when X runs, within the 100 ticks of X's own ticks |
| 15 | Rider's chunk and root's chunk differ (a rider overhanging a chunk line) | `holder` and the holding chunk are the root's |
| 16 | Two players aboard, or a pending second rider | The leaver is put off; U stays |
| 17 | N's link to H lost before `Spawned` | `Present` enters N with the riding word |

## 9. What ADR-0020 and the format need

**ADR-0020**, "Amended with step N7h": the record's `unit` (section 2); `Has`'s `unit`
and `took`, section 4's rows and the transform (section 3); `Enter`'s `unit` and the
root's holder; step 7 as section 5; the riding word in sections 4.1 and 4.3; section 8's
rows in section 10; the N5 amendment narrowed to `end_player`'s paths and section 3's
"otherwise". **ADR-0028**: the store can make one of a commit's arrivals from its note
(section 5); its N5 amendment's named difference goes.

**The format.** A note kind 3, `Has` with a unit and a taken key (u64, 0 for none),
followed by the unit as `entities.rs` writes an arrival's entities; kinds 1 and 2 read
as now, so an older log reads unchanged. The players' file's version is raised: a record
gains a byte for a unit and the unit after the place; the old version is read with no
units. `LoggedPlace` stays `Copy`; the store's `Record` and `PlayerRecord` become
`Clone`, and `Taken::Raised`/`Again` carry the unit. `StoreReply::Enter`, `Entered`,
`Spawned` and `Present` change, so the wire number is raised. No `STATE_FORMAT` change:
`PlayerRiding` and `Pending` are as N5 made them.

## 10. Tests, by someone who did not build it

**The invariant**, asserted after every step: each member key is in exactly one of a
chunk's entities, a filed arrival or a record, read from a stopped store.

- **Store**: section 4's table, a row a test; the transform in the logged commit; a
  `took` with a dead note clearing the unit, one for another key changing nothing;
  `Enter` and `Again` with the unit and the root's holder; every path into `fail_log`
  with a unit or a `took`; kills between the leave's commit and `Entering`, between
  `Entering` and its answer, and while the players' file is written; recovery equal to
  the running store; the older players' file read.
- **Sim**: each condition of section 3 (two riders, a pending second rider, a pending
  leaver, a pig in a boat, a cart); `end_player`'s paths; step 7 in each case of section
  5, the seat rewritten, `took`, hops h + 1; a restore after the `took` commit.
- **Runner**: `Enter` with a unit dropped by a merge and said again.
- **Edge**, against scripted regions: `Spawned` and `Present` with `riding` before the
  unit is shown and after; a `Rides` for a view without an entity, then `Present`;
  `set_passengers` to the rider's own client; a unit gone at h and reported again at
  h + 1 within and after 600 ticks; a late report at h passed over.
- **End to end**, every row of section 8 by name, in one process and a cluster: *a
  quick reconnect whose `Entering` reaches the store before the leave's commit leaves the
  pig in its chunk* (X frozen between leave and commit); *a leave from a region being
  taken over*, rows 3 and 4; each process killed at each step; a pinned line between
  place and home; land nobody holds; below the world.
- **Against the official server**, first (open question 1): log out alone on a pig and
  on a boat, and two in a boat; log in; what the client is sent, in order.

## 11. Open questions

1. **Does 26.3 put the unit back at login?** Yes. On 2026-10-21 the probe
   `probe_a_rider_who_leaves_on_a_pig_on_the_official_server`
   (`bin/clustine/tests/mobs_oracle.rs`) had a player on a saddled pig leave: a watcher
   was shown no pig while they were away, and the player came back seated on it, the
   watcher shown it again. The step is wanted.
2. **Below the world**: the unit back to its chunk, empty (here), or with the player to
   the spawn point, which the game cannot do.
3. **H's claim of the rider's holding chunk and the grant made for the unit's arrival**
   in one group: whether the store counts the claim answered by the grant, or answers
   twice; to read in `lanes.rs` before the store's part is built.
4. **`PENDING_FOR` for a re-entry** into a region that is away: 100 of its ticks, which
   it counts once it runs; enough on reading, to confirm by row 14.
5. **Leashes** to and from a member: N5's restore by key for 100 ticks; the game's
   `UNLOADED_WITH_PLAYER` handling is to be read.
6. **A second login while the first rides** (section 3, row 5): put off here; the game
   gives the vehicle to the second. Worth closing?
7. **A rider who leaves in the middle of a crossing** (pending): the unit is left
   behind, empty. The game has no crossing; named.
8. **Recovery disagreeing with the logged form**: argued to arise only from commits an
   `Opened` passes over, never answered; to check.

## 12. The first review (2026-10-21) and what the design is now

An independent review found that the choice (the store keeps the unit with the
player's record) is right and that four places would duplicate the unit or lose the
ride. The design stands amended:

1. **A taking is its own entry, not part of a place note.** `Region::notes` keeps one
   note per player and a let-go note replaces an earlier `Has` in the same tick
   (`region.rs`, `notes`), so a `took` carried by the placing note could be lost and the
   unit kept twice. The home region's commit carries `units_taken: Vec<(PlayerId,
   EntityId)>`, written outside the note map, logged with the commit, and applied by the
   store with no stay test.
2. **The unit is made whole in the home region only where it would stay there**: the
   predicate `send_entity`'s decision uses (ADR-0028 section 5: the chunk the region's
   own, loaded, neither a copy, lent, lending nor being returned); anywhere else it is
   sent as an arrival, and the player is seated at once only in the first case.
3. **A removal's `Has` is ordered before the `Entering` of the same player** in a tick's
   notes, so that a reconnect whose leave and join the same region applies in one tick
   keeps the ride (either order is safe for ADR-0020's table).
4. **A late unit** (the review's 4b) for every failure-driven reconnect, where the new
   stay's `Entering` reaches the store before the leave's commit: a note `Dead` as
   `Replaced` while the record holds no unit still puts its unit in the record, and the
   store answers the home region with `Unit { player, stay }`. The home region alone
   takes it, by `units_taken`: kept for the entry if the stay is entering, else seated
   or sent back. The store says it again only when the home region's first tick names
   the stay and the record still holds it, as `Again` works. This also closes a second
   login while the first rides.
5. **The store's decision**: a pure trial over all the commit's notes in order, against
   the records as the group's earlier commits leave them, before the commit's
   `nobodys` is worked out, so a unit sent to land nobody holds gets its `Granted` record
   ahead of it; the unit's arrival takes the index after the region's own arrivals.
6. **Recovery does not judge again**: a note's unit goes into the record and a taking
   clears it as logged, with no judgement in `apply` (the review checked that replay
   equals the running store: `Log::fail`/`settle`, `make_over`).
7. **The unit's own seat for the leaving player is written seated**, so the take-in
   drops it, and the returning player is placed at the dismount pose where the unit is
   not put under them; the pig's blink for watchers when it is sent back is named.
8. **Until the unit is introduced the client is in the air at the seat pose** while
   the home region ignores its moves: none in the common case, and in a long wait it
   ends with a teleport, named.
9. **The format**: the players' file keeps a unit with the versioned entity codec of
   chunks and arrivals; a players' file written before is read without units.

The open questions, answered: 1 yes (the probe); 2 back to its chunk, empty; 3 the
claim is answered twice, harmlessly (the worker keeps arrivals by number), both orders
to be tested; 4 100 ticks suffice (ages counted by the region that takes them); 5 the
leashes among members hold by key; a leash to a knot or from an outside mob is kept in
the unit and broken at the outside end as the game does, after a probe of
`UNLOADED_WITH_PLAYER`; 6 closed by item 4; 7 named; 8 confirmed, the error path goes.
A second review follows before anything is built.
