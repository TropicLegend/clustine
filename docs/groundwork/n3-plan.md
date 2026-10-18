# Step N3: entities crossing a line, planned

Draft of 2026-10-18, revised the same day after an independent review (section 5), whose
changes to the contracts and the store are to be read again before they are built. Step N3 of
[ADR-0028](../adr/0028-entities.md), sections 5 and 6 as accepted after five reviews,
built after N4 (an entity must move before it can cross). The record decides the rules;
this plan says where each goes in the code as it is now, and in what order. It is not
delegated (the store's and the edge's ordering, CLAUDE.md), but its tests are written
from the record by someone who did not build it.

## 1. What a player meets after N3

- An item thrown, washed or pushed across a line between regions goes on across it, at
  its own pace while the two regions' border ticks in lockstep, a tick or two late where
  it does not; it is neither lost nor doubled however the regions, the store or the
  edges crash meanwhile.
- A Q pressed right after crossing a pinned line throws (today it is refused): an item
  thrown into another region's chunk goes as an arrival.
- A falling block that falls into another region's chunk lands there.
- Where regions follow players, entities working at their line make them merge at once
  (ADR-0028, section 7), as circuits do.
- Not in N3: vehicles (they come in N5, and their crossing with riders, section 5 of the
  record, with them); effects on entities across a line (hits, pushes: there are no
  living entities until N7, and items are picked up in their own region only).

## 2. Parts, in order

### N3a, the contracts

- `clustine-world`: `Arrival { id: EffectId, chunk: ChunkPos, entity: Entity }`.
- `clustine-rpc`: `StoreRequest::Commit` gains `arrivals: Vec<(ChunkPos, Entity)>` (sent
  by the tick) and `arrivals_taken: Vec<EffectId>`; `StoreReply::Arrivals(Vec<Arrival>)`
  on the stream.
- `clustine-format`: the commit's kind 16 carries them (an entity encoded as in N2b); log
  records `ArrivalsDropped(Vec<EffectId>)` and `ArrivalsForward(Vec<Arrival>)` (split over
  records where large, ADR-0028 section 5, "Large"); the format raised.
- `clustine-sim`: `TickInputs::arrivals: Vec<Arrival>`; `TickOutput::arrivals: Vec<(ChunkPos,
  Entity)>`, `TickOutput::arrivals_taken: Vec<EffectId>`, `TickOutput::arrival_tickets:
  Vec<ChunkPos>` (chunks the region holds and wants loaded for an arrival);
  `RegionEvent::HandedOn { entity, chunk, hops }`.

### N3b, the store (`services/worldstore`)

An `Arrivals` beside `Filed` (`effects.rs`): by id, by chunk and by living segment, with
an undo, as effects are; filed **by the chunk's holder** (`Table::holder`), not by use.

- **Commit** (`lanes.rs`, the `StoreRequest::Commit` arm): before anything is appended,
  each arrival's chunk is looked up: held by a region, filed for it; held by nobody (only
  without pins), granted to the sender: `LogRecord::Granted` appended before the commit,
  `table.grant`, `Change::Granted` in the group, `Claimed { granted }` pushed to the
  sender's `owner.unsynced` so that it reaches the worker before the stream. Then the
  commit is appended, the arrivals filed (`Undo` in the group), put on the holder's stream
  as `Streamed::Arrival(id)`, and the commit's `arrivals_taken` checked: each must be filed
  and held by the committing region (`table.holder`), and a commit naming one that is not
  is refused as malformed (the region is lost, `lose`), never passed over; the taken are
  dropped with an undo.
- **The stream** (`end_group`): `Streamed::Arrival(id)` resolved through the arrivals, kept
  only where `table.holder(chunk)` is still the region, batched as `StoreReply::Arrivals`
  with the mark after.
- **Sent again** to the holder: at its hello (`admit`: every filed arrival whose holder is
  the region, pinned land included), behind `Absorbed` at a merge (those whose holder the
  merge made the survivor), at a split's part's hello (by holder), with a lend's grant
  (`lent`: those of the granted chunks, for the borrower), behind `PinnedBack` at a lent
  chunk's return (`returned`: for the holder the release leaves), at a claim that grants a
  chunk with arrivals filed, and at a return or a lend called off (the chunks taken up
  again).
- **A `Return`** of a chunk with arrivals filed, where `table.pinned_holder` is none: the
  chunk is left out of `returning`, the region is answered `Claimed { granted: [chunk] }`
  and the arrivals sent again; a chunk being returned whose arrival comes meanwhile calls
  the return off as a claim does. A `Return` of an unreadable chunk goes on whatever is
  filed (the record: it holds no ticket).
- **Dropped** only when taken. `end_uses` drops no arrival. `begin_uses` at the store's
  start drops none.
- **Written forward** with a segment as effects are (`collect`, `write_forward`, the
  segments kept alive while arrivals live in them), as `ArrivalsForward`.
- **The start** (`load`): commits' arrivals filed from the log, `arrivals_taken` and
  `ArrivalsDropped` remove, `ArrivalsForward` files again.
- **A cut group** (`fail_log`): the arrivals' undo with the effects', the grant undone with
  `Change::Granted`.
- **The fold** at a change of division (`make_over`): untaken arrivals written as
  `Replayed::Entity(chunk, Whole(entity))`, their `ArrivalsDropped` synced before the
  `Opened` records.

### N3c, the worker (`services/worker`)

- `Border` keeps arrivals by id (`take`), as effects; an arrival is ready when its id is
  at or below the mark and its chunk is loaded (no 3x3), and is handed to the tick in
  `inputs.arrivals`.
- **What it keeps** (the record, "What a worker keeps"): arrivals of chunks the runner
  holds or is about to hold, judged from the store's replies it has received
  (`inputs.granted` not yet given to a tick, a `Lent` or `PinnedBack` to come), not from
  the sim's knowledge alone; dropped for a chunk it sends in a `Return` or a `Lending`.
- **Taken once**: ids handed to a tick are kept as taken until the tick's commit is
  `Committed`, so that an arrival sent again in the meantime (a call-off, a `Return`'s
  `Claimed`, a `LendCalledOff`) is passed over; after it, the store has dropped them.
- The commit carries `output.arrivals` and `output.arrivals_taken`, and is needed when
  either is not empty; `output.arrival_tickets` asks for those chunks' loads.

### N3d, the sim (`crates/clustine-sim`)

- **Sending** in place of N4's guard (`View::may_hold`, `move_guarded`, `move_guarded_as`):
  an entity whose move ends in a chunk the region does not have loaded and ticking is
  sent, not held, where the chunk is another region's (`sends_to`'s rule: lending,
  `Foreign`, or `Unknown` and not wanted and not pinned), or the region's own and not
  loaded: `Gone` committed in its chunk, the entity whole in `output.arrivals` with the
  `hops` it had, `RegionEvent::HandedOn`. An own chunk held and not loaded gets an
  arrival ticket. The guard stays only where a move would end in a chunk asked for and not
  yet granted (`Asked`): the entity waits at the line until the answer.
- **Throwing** into a chunk the region does not tick throws as an arrival there (the
  finding of N2's tests).
- **Taking** after `run_seams` and before the entities' phase: each arrival of
  `inputs.arrivals` whose chunk is loaded is added with `hops` one higher (a `Whole`
  committed), ticks in that tick, and its id goes to `output.arrivals_taken`; the record's
  rule for a key the region has already (a malformed stream) refuses it.
- **Arrival tickets** as lend tickets (`lend_tickets`): a chunk held for an arrival is
  loaded, counts as used for `settle_chunks`, and its ticket ends 100 ticks after the
  arrival is taken; a chunk held only for an arrival (granted with it) is given back when
  the ticket ends, not after `return_after`.

### N3e, the edge (`services/edge`)

- `HandedOn { entity, chunk, hops }`: the edge keeps showing the entity, marked handed on
  at `hops`, and never introduces one it does not show; the receiver's report at a higher
  `hops` takes over (`takes_word_of` as now) and continues it on the wire, its tracker
  kept (N4d's take-over).
- A snapshot removes the entities shown in its chunk as handed on that it does not have,
  from whichever region; the removal keeps the key at the handed-on `hops` (`gone`).
- An entity reported in a chunk outside the replica is removed through `refresh_entity`
  (`upsert_entity`'s early return must not skip it).

### N3f, merging at once (`services/coordinator` or the store's reshaping)

As ADR-0027 section 7 does for circuits: where regions follow players, an entity within
four blocks of a line whose holder sent an arrival across it in the last 600 ticks counts
as entities working at the line, and the two regions merge at once. Read where circuits'
"active at the line" is reported and decided, and add entities to it.

## 3. Tests

From the record and this plan, by someone who did not build them: an item crossing a
pinned line both ways, coupled and uncoupled, is there once on the far side, at the
game's position for its motion; a crash of the sender, of the holder or of the store at
every step of a crossing (before and after the commit, the stream, the taking) leaves it
once; a return, a lend, a merge, a split and a claim while an arrival is in flight; land
nobody holds (no pins): the sender is granted it; an unreadable chunk; a fold at a change of
division with an arrival filed; the edge shows it once by one wire id throughout, through
a snapshot taken mid-crossing, and with two edges.

## 4. Questions for the reviewer

- Is the order "Granted, then the commit" with the holder looked up before the append right
  against `admit` and `load` (the table's records are replayed from `table.from`)?
- Is "taken until `Committed`" in the worker enough against every resend, given that a
  region's restart is a fresh session?
- Does sending in place of the guard leave any path where an entity is in no durable state
  (sent, then the sender's commit cut)?
- Is anything a player meets within minutes missing?

## 5. The review (2026-10-18), and what changes

Not ready as written, ready after these; they replace the sections above where they
differ.

- **Checked before anything is appended** (its D1): the takings deduplicated and checked by
  holder first, then the grant lookups and the `Granted` records, then the commit; a refused
  commit appends nothing.
- **The store's start and `Opened`** (D2): a commit that a later `Opened { restored }`
  excludes was "counted as one that was" for effects; for arrivals that doubles or loses.
  While the log is read, each commit's arrivals and takings are kept by (region, tick); at
  an `Opened` those above `restored` are taken back; the rest are applied at the end.
- **The arrival ticket comes from the runner** (D3): the sim never sees an arrival waiting
  for its chunk. The tick gets `TickInputs::arrivals_waiting: Vec<ChunkPos>`: the chunks of
  kept arrivals that the runner holds or is about to hold, not loaded, and not lending,
  returning or unreadable; the sim tickets those, and the ticket counts in `wants()`
  (so that an `Unknown` pinned chunk is claimed before it is loaded) as well as in
  `settle_chunks`. `TickOutput::arrival_tickets` goes.
- **Coupled crossings are due** (D4): an `Arrival` carries a `Due`, made by the store from
  the couplings as effects' are (`couplings.due`), and `Border` holds it to it, so that a
  coupled holder does not take a crossing in the tick it already ran, which would tick the
  entity twice in it.
- **Arrivals handed in and not taken** (D5) go back to the runner's kept ones: those handed
  to a tick and not in its `arrivals_taken` are put back.
- **Sent wherever the chunk is not loaded** (C1, C2): into another region's chunk (lending,
  `Foreign`, or `Unknown` and not wanted and not pinned), into the region's own chunk not
  loaded (`Held` and not in `chunks`, or `Unknown` and pinned and not lent away), into one
  asked for and not yet granted, and into one `Unknown`, not pinned and wanted; never held
  at the line. Into an own chunk loaded but not ticking the entity moves in and stops
  ticking, as the game's; into an own chunk loaded and full, N4's guard stays (G1).
- **A taking into a full chunk** is taken all the same (an arrival cannot be refused);
  `MAX_CHUNK_ENTITIES` binds throws and drops only (G1).
- **Pistons** send from the block entities' phase; the `Gone` and the arrival go in that
  tick's commit, and the entity's memory, draws and index are dropped; the test that held
  an item at the line is replaced (G1).
- **A throw as an arrival** takes a key with no room check and `hops` 0, reports no
  `HandedOn`, and appears where it is taken (G2).
- **A return called off from the commit** (G3): an arrival filed for a holder whose chunk is
  in its `returning` removes the chunk from it, answers that holder `Claimed { granted }` in
  the group and begins its use again, also from the `Return` arm, where `end_uses` has ended
  it; only for chunks with no pinned holder (a borrower's return goes by `PinnedBack`).
- **Resends in a group are ids** (G4): `Streamed::Arrival(id)`, resolved at the group's end,
  never a copy, since `Committed` goes before the stream and a copy taken by a later commit
  of the same group would be taken twice; only `admit` and `absorb`, which end the group
  first, answer at once.
- **What the worker drops** (G5): kept arrivals of chunks it stops holding (those a split
  gives the part, an absorbed region's, those under `Lent`); "a `Lent` or `PinnedBack` to
  come" reads "received and not yet given to a tick".
- **A key the holder has already** (G6): the runner gives up, as at `NotHeld`, and leaves
  no arrival filed and untaken.
- **The fold** (G7): arrivals' `Replayed::Entity` bypass `replayed()`'s holder filter, go
  after all the commits' changes, and their `ArrivalsDropped` before the `Opened` records in
  the same sync.
- **Merging at once** (G8): reported as circuits are (`redstone_at_work` in the worker, from
  the region, into `PlayersOf::redstone`) by a `Region::entities_at_line()` the sim keeps for
  600 ticks per line it sent across; decided by the coordinator's policy beside
  `Why::Redstone`; the survivor goes on reporting those chunks so that a later split keeps
  clear of the spot (`state/follow.rs`'s rule), with the cap of reported chunks shared with
  circuits.
- **The edge** (G9): `HandedOn` carries the entity's pose at the line and the edge takes it;
  a sender's own resume snapshot does not remove what it handed on; the taking's
  `EntitySpawned` comes before that tick's moves; the new removal applies only to entities
  shown before.
- **Named differences**: an item crossing back and forth along an uncoupled line stands
  still for one to three ticks at each crossing and snaps back on the client; items on
  either side of a line do not merge (merging is a region's own: it waits for entity effects
  across a line, with N7).
