# ADR-0026: Redstone

- Status: Proposed on 2026-10-17, from the plan of phase R
  ([r-plan.md](../groundwork/r-plan.md)), after two independent reviews of that plan
  whose findings are worked in (its sections 8 and 9), and the owner's answers to its
  questions (its section 6) and to the ways of crossing a line
  ([r-lines.md](../groundwork/r-lines.md), reviewed). The first review of this record
  found it ready after named changes, which are made ("Review"); a second, focused
  review accepted it after five small changes, which are made. **Accepted** on
  2026-10-17. Where the plan and this record differ, this record holds.
- Date: 2026-10-17

## Context

Since phase B ([ADR-0024](0024-block-behaviour-in-a-region.md)) blocks behave inside a
region, and since phase E ([ADR-0025](0025-effects-between-regions.md)) what a tick
does crosses between regions. Nothing is powered: a lever clicks into place and does
nothing, dust never lights, a door beside a lever stays shut, a piston never moves,
and the server sends no sound. Creative players build exactly these first.

The owner agreed on 2026-10-15 that redstone is a phase of its own after B and E, and
answered the plan on 2026-10-17: what needs entities or inventories waits for their
phase; at a line between regions a crossing a tick late is not enough, and pistons
are to move across. Of the ways weighed (r-lines.md), the owner chose **lending at
build time** at a pinned line, which is a record of its own (ADR-0027); lines between
regions that follow players almost never tick (ADR-0017, sections 3.5 and 3.6), and
where they do, ADR-0027 is to merge the regions at once. This record is redstone
inside a region and what it needs of the store, the edge and the simulation; section
10 is what holds at every line until ADR-0027 is built.

The reference for the game's 26.3 is SteelMC-26.3 (`steel-core/src/behavior/blocks/
redstone/`), and the jar's classes wherever order matters. 26.3's wire is the game's
default evaluator; the orientation-based one comes only with the Redstone Experiments
pack.

## Decision

### 1. Signals

The level's queries are the game's (`getSignal`, `getDirectSignal`,
`hasNeighborSignal`, `getBestNeighborSignal`, `getControlInputSignal`,
`isRedstoneConductor`), each answered by the block's own rules, with "no wire
signals while wire measures" as a flag of the query, not a global. They live beside
the blocks' other rules (`clustine-blocks`), read through the region's `block()` (so
copies are read as loaded chunks are), and are pure functions of the blocks.

**Where a rule that reads a signal runs.** Redstone reads up to two blocks from the
block whose rule runs (wire its diagonals; a conductor's direct signal its own six
neighbours; a comparator two behind), quasi-connectivity three (the block above's
neighbours, through a conductor), and a piston's structure 13. All of it lies within the three by three chunks around the
block. One predicate, **the chunk ticks** (`update_ticking`, ADR-0024 section 2, which
counts copies beside chunks with viewers' or guests' tickets), decides where such a
rule runs: an update of a block whose rules read a signal, a seam entry or an effect
run again (whose test today is the chunks of the target's six neighbouring blocks;
for these blocks it becomes this one), a block event, a ticker, and the rules a click or a placement
runs. "A block whose rules read a signal" is a property of its class in
`clustine-data` (wire, diodes, torches, observers, pistons, lamps, doors, trapdoors,
gates, rails, note blocks, bulbs, bells, targets, dripleaf, redstone ore). An update
that cannot run is kept as a seam entry until its chunk ticks: the updater already
turns one step of a chain into an entry and carries on, so the order of the chain
that runs is the game's; the kept entries run later in their key's order, one a
target, as kept entries do today. A click or a placement that cannot is refused, as
one into land not loaded is; the predicate a click meets is the one worked out at the
end of the tick before, since inputs run before it is worked out again. So no rule reads a missing block as air, and nothing settles dark beside
power for good when copies come and go.

### 2. Updates

- The updater gains the game's entries: a **single target** with its source block
  and the update's flags, among them the game's `UPDATE_MOVE_BY_PISTON`
  (`neighborChanged(pos, block, orientation, movedByPiston)`; the orientation is
  always none without the experiments), **the six neighbours except one side**
  (`updateNeighborsAtExceptFromFacing`), **indirect shape updates**
  (`updateIndirectNeighbourShapes`, for wire's diagonals), and **the output-signal
  update** of a block with an analog output (`updateNeighbourForOutputSignal`). Every
  `neighbour_changed` is given the source and the flags.
- **An output-signal update** reaches a comparator two blocks away through a
  conductor, having read both: kept as an entry or carried as an effect, its direction
  is towards the block it came through, which is where it is kept. Two kept entries of
  one key keep the later, whatever their sources and flags, as kept entries do today.
- **No stored format changes for them.** A kept seam entry and an
  `Effected::Update` are already one step to one target, with its source block and
  its flags (`Seam`, `clustine-world/src/chunk.rs`): a single update and each step of
  the six-but-one are kept as the `Neighbour` steps they are, and moved-by-piston is a
  bit of the flags they keep.
- **Wire is the game's default evaluator**, checked against the jar's
  `DefaultRedstoneWireEvaluator` and `RedStoneWireBlock.calculateTargetStrength`, not
  against a port alone (SteelMC's is shaped as Lithium's). It sets itself with
  `UPDATE_CLIENTS` and then calls `updateNeighborsAt(position, wire)` on each of seven
  positions, the wire and its six neighbours, each of which tells that position's six
  neighbours, in the iteration order of Java's `HashSet<BlockPos>` of 16 buckets: the
  hash `(z * 31 + y) * 31 + x`, spread as `HashMap` spreads it (`h ^ (h >>> 16)`), then
  `& 15`, insertion order within a bucket, inserted as the wire, down, up, north,
  south, west, east.

### 3. Ticks and the phases of a tick

- `has_scheduled_tick(pos, block)`: a tick of that block pending at that position,
  false for one already drawn to run this tick. `will_tick_this_tick(pos, block)`: a
  tick drawn to run this tick and not yet run. Each drawn tick leaves that set as it
  runs, and it is empty outside the phase of block ticks. Torches and diodes depend on
  the difference.
- **Handling a tick** is true from the start of the region's tick phases through the
  block events, the seam phase and effects included (an update that arrives as an
  effect was made while its maker handled its tick), and false while inputs, entities
  and block entities run, as the game's `isHandlingTick`. A piston chooses an instant
  retraction by it and by whether it ticked this tick (`last_ticked`, the region's
  tick number; not saved, as the game does not save it, and forgotten at a restore).
- The tick's phases become: inputs, seams and effects, block ticks, fluid ticks,
  post-processing, random ticks, **block events**, **block entities**, and then the
  departures of players worked out (today after the random ticks), so that a player
  a piston pushed over a line crosses in the same tick. Entities join later, between
  block events and block entities, as in the game.

### 4. Block events

- A block event is `(position, block, kind, data)`. The phase runs a queue in
  insertion order, exact duplicates dropped, to empty, events queued while it runs
  included; an event whose block is no longer there does nothing; one that runs is
  sent to players within 64 blocks.
- An event whose chunk does not tick is put back, in order, for the next tick; one
  queued after the phase (by a moving piston that finishes) waits for the next, behind
  those put back, which the game puts back at the end of the phase. A running region keeps the queue in memory, in its order, as the game does. **An
  event that outlives its tick is also kept in its chunk**, in queue order, committed
  whole per chunk with the tick as the chunk's scheduled ticks are (ADR-0024, section
  4) and only in a tick in which they changed, so that a chunk that does not tick is
  not rewritten every tick, and so that it is carried by the chunk through merges, splits, returns, saves and
  loads by another region, and outlives a crash, which the game's queue does not. A
  chunk unloaded takes its events out of the region's queue; a chunk loaded puts its
  kept events at the queue's end. A restore, a merge or a split builds the queue from
  the chunks' kept events, by chunk and then in each chunk's order: the order of two
  regions' events never had a game's order to match, and this one is deterministic.
  No counter is needed, as scheduled ticks need none (`ticks.rs`).

### 5. Block entities

- **Typed block entities** for what the simulation reads or ticks: the comparator
  (its output) and the moving piston (the moved state, its direction, extending or
  not, whether it is the source, its progress), both durable, serialised into the
  chunk's NBT bytes as the game writes them and read from them; and the bell, whose
  ring and shaking are not in the game's NBT and live in memory only, as in the game
  (its search for raiders waits for entities). The rest stay opaque.
- **Tickers** (moving pistons, bells) run in the game's list order, which a running
  region keeps in memory: in the order they were made; those made during the phase
  wait for the next tick; a chunk's tickers leave the list when it is unloaded and
  join it at its end, in position order, when it is loaded, as the game drops them at
  an unload and adds them at a load. A restore, a merge or a split builds the list by
  chunk and then position. A ticker in a chunk that does not tick waits.
- **The moving piston ticks three times**: it is made in the block-event phase and
  ticked in the same tick (progress 0 to 0.5), the next (0.5 to 1) and the next, which
  finishes it, each at its own position (confirmed against the jar's
  `PistonMovingBlockEntity.tick` in R5, and corrected here if it differs). The game
  saves only the progress before a tick, and a chunk loaded sets both to it; so a move
  carried half way through a reload, a restore, a merge, a split or a move between
  workers takes a tick more, as the game's does after a chunk is reloaded.

### 6. Commits carry one ordered list of changes

A commit's `changes` become a list of `Block(position, state)` and
`BlockEntity(position, data)`, in the order they happened in the tick; replay applies
them in order, as the chunk's own `set` does (a change of block drops the block
entity of another kind, makes an empty one of a new kind, keeps one of the same kind).
Where the game makes a fresh block entity for a block of the same kind (a block
replaced by another of its kind), the simulation writes the empty block entity
explicitly after the block. Replay stays idempotent: each change sets a definite
state and the last wins. The same list goes everywhere block changes go today:

- the log and its replay at a restore (`worldstore/src/chunks.rs`), and the store's
  `put` of a commit into its chunks;
- copies' updates (`CopyUpdate.changes`): a change of a block entity alone advances a
  copy's chain as a block change does;
- the worker's record of unsaved and unchanged chunks;
- the region's events to its edges and the edge's replica, whose chunk packets then
  carry a moving piston's data (`BlockEntity` changes are not sent to clients as
  block-entity data unless the game sends them; R2c settles which, against the
  official server).

The store's crash tests are extended to a block entity whose kind changes across a
checkpoint, and to a crash at every point of a commit with both kinds of change.

### 7. Clicks and placement run the blocks' rules

- **A click runs the block's own `use` rule inside the updater**, in place of a list
  of states set all or nothing (`Clicked::Used`): the lever's update of its support's
  neighbours, the button's release tick, the comparator's mode and output, the
  repeater's delay, the note block's note and block event, wire's dot and cross,
  redstone ore lit, and the doors and the rest as now. A click refused by section 1 or
  by land not held changes nothing. A left click on a note block plays it; a player
  stepping on redstone ore lights it.
- **Placement runs `onPlace` and `setPlacedBy` and reads signals**: a lamp, a door, a
  trapdoor or a rail placed beside power comes out powered; a piston placed powered
  extends; a repeater beside power schedules its tick.
- Inputs run outside "handling a tick" (section 3).

### 8. What the client is sent

- `RegionEvent::BlockEvent { position, block, kind, data }`, sent as the game's
  block-event packet to players within 64 blocks.
- **The order of the game**: the game broadcasts a tick's block changes in its chunk
  tick, before its block events, and the changes made from the block-event phase on
  go in the next tick's broadcast, coalesced per position with that tick's; the
  client re-runs each move from its block event on its own world. A region's events
  carry one mark a tick, "block events begin". Events before it go as today: sounds,
  level events and the rest at once, the block changes collected; at the mark the
  edge sends the changes collected, and from then on sends block events, sounds and
  the rest at once, in their order (a piston's sound before its block event, as the
  game plays it), and collects the block changes after the mark for the next tick's
  batch. The mark has no position and passes every subscription. **The carried
  changes always go**: a region whose tick had block changes after the mark sends a
  delta for the next tick even when that has nothing else, and the edge sends what it
  carries for a region when its link to it ends, when the region is absorbed, or when
  it sees a later tick of the region. The order is checked against the official
  server.
- `RegionEvent::Sound { sound, category, position, volume, pitch, except }` and
  `RegionEvent::LevelEvent { kind, position, data }` (a torch burning out smokes and
  hisses), sent as the game's packets to the players the game sends them to: a sound
  within 16 times its volume of blocks, a level event within 64. `except` is the
  player whose own client plays the sound: a lever's click leaves out nobody; a
  button's, a door's, a comparator's, a block placed or broken by a player leave out
  that player. Every block's sound is checked against the jar. The sound's seed is
  drawn by the edge, not by the simulation's rules, so that no draw of a rule moves.
  The sounds of what players and phase B do come in this phase too: a block placed or
  broken by another player, a door opened by hand, water and lava, a block that breaks
  under an update.

### 9. Pistons

- The game's resolver: a limit of 12 blocks, slime and honey that do not stick to
  each other, push reactions (immovable, destroyed, pushed, pushed and pulled), blocks
  with block entities immovable, quasi-connectivity (the five sides but the face,
  then the piston's own position downward, then the block above), the block events
  (0 to extend, 1 to retract, 2 to retract at once when the block two ahead is a
  moving piston extending the same way and it is less than half way, has ticked this
  tick, or the level handles its tick), sticky pistons that drop their block on a
  short pulse.
- **Players are pushed and launched.** The client owns its player's position: the
  edge checks a move only for numbers that are not numbers and for an unconfirmed
  teleport, and the region takes the position. The client pushes itself by its own
  moving piston and is launched by slime on its own. So the region's push of a player
  decides only where the region holds the player between the client's packets, which
  others see, and it never sends that player's client a correction or a motion,
  which would pull them back. A push that carries a player over a region's line is a
  crossing as any walk over it. Other entities wait for their phase.

### 10. At a line, until ADR-0027

- **Between regions that follow players**, the regions are to be merged at once when
  redstone is active at their line. What tells the coordinator, for how long, and
  what keeps a split from drawing the line through the same circuit again (a line
  there ticks mostly at trails and guests, where players can be further apart than a
  split's distance) belong to ADR-0027, with the lending, and are not built before
  it.
- **At every line**, pinned or between regions that follow players, until ADR-0027 is
  built: a signal across the line is read
  from the copy, and a change across it reaches the other side as an effect, a tick
  late while coupled, a tick or a few late while uncoupled, given up or preparing, when
  more can differ. Repeaters, torches, comparators and observers keep their timing
  while coupled (a scheduled tick made by an effect counts from the maker's tick);
  wire, lamps, doors, rails and pistons are a tick late per crossing; dust across the
  line goes dark over 0.75 to 1.5 s and cannot oscillate; a circuit whose paths cross
  differently can end otherwise than in the game.
- **A piston moves only within land it holds** until then: every position its move
  sets (where blocks go, those destroyed, the head, the moving pistons an instant
  retraction finishes) and every block it moves lies in a chunk that ticks and that
  the region holds, or it does not move. The game checks only when a piston is
  updated or placed, so a piston refused would stay put while powered; its check is
  kept in the piston's chunk as **a waiting piston** (its position), committed with
  the chunk's seams, and tried again only when the region's land changes (a grant, a
  merge's or a split's record, a lend) or a chunk its move reaches starts ticking,
  never by the seam phase's retries, so that a
  piston refused at a pinned line does not rewrite its chunk every tick. A move is
  never turned into conditional sets in another region's land.
- Plates and tripwire see the region's own players; block events and moving pistons
  are the region's own.

### 11. What is built, and what waits

Built here: levers; buttons pressed by hand; the redstone block; torches with
burnout; plates and tripwire pressed by players; redstone ore; wire; repeaters
(delay, locking); comparators (modes; the analog outputs of blocks without an
inventory: cake, composter, cauldrons, end portal frame, jukebox's state, respawn
anchor, beehive's level); observers; target blocks by signal; lamps, doors,
trapdoors, gates, note blocks, copper bulbs, powered and activator rails, rails'
shapes by power, big dripleaf, bells; pistons and sticky pistons.

Waiting for entities: TNT, dispensers, droppers, hoppers, crafters, sculk sensors and
shriekers, detector rails, arrows on buttons and targets, plates and tripwire pressed
by anything but players, pistons pushing anything but players, comparators reading
containers. The daylight detector waits for light in the simulation.

### 12. Durable state

- Block and block-entity changes, every tick they happen, in one ordered list
  (section 6).
- Block events that outlive their tick, kept in their chunks, and the moving pistons
  and comparators, block entities of their chunks; the order of the queue and of the
  tickers is the running region's, rebuilt at a restore (sections 4 and 5).
- Redstone's scheduled ticks are chunk ticks as any.
- Not durable, as in the game: the torch's burnout list (by region tick numbers,
  dropped at a reshape and a restore), `last_ticked`, the set of ticks drawn this
  tick, a bell's ring.
- Kept beyond the game: block events that outlive their tick, through unloads and
  crashes (the game keeps its queue in memory only), and waiting pistons.

## Consequences

- Circuits inside a region behave as in the game, in timing and in order, wire's hash
  order included; a lever opens a door, a clock ticks, a piston door works, a slime
  launcher throws its player.
- The server sends sounds, and the client sees pistons move as the game's.
- A commit can carry block entities, so the log, replay, copies and the edge carry a
  new kind of change; busy circuits commit every tick they change (measured in R8).
- At a pinned line, until ADR-0027 is built, redstone crosses a tick late and pistons
  do not cross (section 10); between regions that follow players, ADR-0027 is to
  merge them at once when redstone reaches their line, and until then the same holds.
- What needs entities waits for their phase, which comes next.

## Steps

As the plan's section 4: R1 this record, reviewed; R2a the store (the ordered list of
changes, formats versioned), R2b the simulation's ground (updates, ticks, phases,
block events and tickers in chunks, typed block entities, clicks and placement,
section 1's predicate, a piston's kept check), R2c the edge (packets, sounds, the
order of block events), all three by me; R3 signals, sources, wire and consumers, R4
torches, diodes, observers, targets, by subagents in turn; R5 pistons, with players,
by me; R6 the sounds of phase B, by a subagent; R7 tests from this record by someone
who did not write the code; R7b ADR-0027, lending at a pinned line and merging at
once, by me; R8 comparisons with the official server and measurements, and what to
try with a real client.

## Found while building

R2a, the ordered list of changes (2026-10-17):

- **The list is `clustine_world::Change`**, `Block` or `BlockEntity`, carried by
  commits, copies' updates, the log and the store's replay (`Replayed::BlockEntity`)
  in the order a tick made the changes; `Chunk::apply` applies one as the chunk's own
  `set` and `set_block_entity_data` do.
- **The log writes it as kind 13**, only for a commit with a block entity's data
  among its changes: the fields of kind 10, then each block entity's change with the
  number of block changes before it. Commits of blocks alone are written as before,
  so older logs and older readers of such commits are untouched.
- **The simulation says a block entity's new data as
  `RegionEvent::BlockEntityChanged`**, in its place among the tick's
  `BlockChanged`; the worker commits both in that order, and the edge keeps the data
  in its replica. Nothing emits it before R2b.
- Data a chunk refuses (more than `MAX_BLOCK_ENTITY_DATA` or the chunk's whole) is
  refused alike in the region's chunk, a copy, the edge's replica and a replay, so
  all keep the block entity without it.

R2b, the simulation's ground, first part (2026-10-17):

- **The updater's entries are the game's three**: the six neighbours (`Neighbours`,
  with a side to skip for `updateNeighborsAtExceptFromFacing`), a single block
  (`Single`), and a shape asked again. A multiple update keeps the block that changed
  as it was when the update was made, the game's `sourceBlock`, where it read the
  block afresh at each step before; a kept entry carries it as its source, and
  moved-by-piston as the game's flag 64.
- **`has_scheduled_tick` and `will_tick_this_tick` answer of the queues**, which hold
  only the ticks of chunks that tick; a chunk that does not tick keeps its ticks in
  itself, where redstone's rules never ask.
- **A test of E6 raced**: `a_handshake_under_way_survives_a_restart_of_the_store` took
  the disk as a crash leaves it after dropping its regions' handles, and a handle
  closed ends its region's couplings and calls off its handshakes, which a crash does
  not. Whether the store had done so first depended on scheduling: 32 of 200 runs
  failed twelve at a time. The disk is now taken while the handles are open.

R2b, second part, block events (2026-10-17):

- **Kept events travel as seam entries do**: `Chunk::events`, a chunk file's bit 2 of
  its flags and a part after the seams (`kept_events.rs`), a commit's `events` of the
  chunks whose kept events changed, whole, which the log writes as kind 14 (the
  fields of kind 13, then the events), and the store's `Replayed::Events`. A chunk or
  a commit without any is written as before.
- **The queue is the region's in memory, and the chunks are its durable form**: at the
  end of each tick every loaded chunk keeps the events of its blocks still queued, and
  a chunk whose list changed is named in `TickOutput::events_changed`, which the
  worker commits as it commits seam entries. A chunk unloaded takes its events out of
  the queue; one loaded, or every chunk of a region restored with its chunks, puts
  them at the queue's end in ascending order of the chunks.
- **No block has a `triggerEvent` yet**: the phase runs and puts back, and an event
  that reaches its block does nothing until R3's note blocks and bells and R5's
  pistons. `RegionEvent::BlockEvent` is what a run event will say, which the edge
  sends from R2c on.
- **Typed block entities and their tickers come with their first users** (the bell in
  R3, the comparator in R4, the moving piston in R5), each with tests of its own, and
  so does the phase of block entities, which has nothing to tick before them.
  `Region::set_block_entity_data` is how a rule gives a block entity new data, said
  as `BlockEntityChanged` in its place among the tick's changes.

R2b, third part, clicks and what may be read (2026-10-17):

- **Section 1's rule holds for neighbour updates, not shape updates.** Signals are read
  in `neighborChanged`, `onPlace`, ticks and uses; a shape asked again reads its
  neighbours alone, wire's and a diode's included, which an update's existing test
  makes sure of. So `may_run_at` (the chunk ticks, for a block whose rules read a
  signal, `clustine_blocks::signal::reads_signal`) is asked of a neighbour told, a
  kept `Neighbour` entry and an effect's, and `readable_around` of a shape. Tests of
  B3 that broke a block in a region whose one chunk does not tick now load the eight
  around it, since a rail, a repeater or wire told there waits as an entry.
- **A use is a list of sets with the game's flags** (`Clicked::Used { changes, flags }`):
  a door and a gate 10, which tells no neighbour, a trapdoor 2, with its water's tick
  where it is waterlogged, a flower pot 3. The game sets a door's clicked half and the
  other follows from its shape; a shape asked again waits where the block's
  neighbours cannot all be read, which at a line without a copy of its other side
  would leave a door open by half, so both halves are set, the clicked one first, as
  the shape would set the other.
- **A click is refused where its use reads a signal** and the chunk does not tick
  (`use_reads_signal`, the comparator's); every other use reads the block itself.
  **A placement's test comes with the first placement that reads a signal**, in R3:
  until then no placement reads one, and refusing doors and rails where a chunk does
  not tick would change W6's placing for nothing.

R2c, the edge (2026-10-17):

- **The sound events are generated** from the jar's registry report
  (`clustine_data::sounds`, named as the game's `SoundEvents`: 1,991 of them), and a
  sound is sent by its registry id, as the game sends one it has registered.
- **`RegionEvent::Sound` and `RegionEvent::BlockEvent` go to the players who have the
  chunk and stand near**: a block event within 64 blocks, a sound within 16, or 16
  times its volume where that is more, never to the player it leaves out. The edge
  draws a sound's seed itself. Level events go the same way, within 64.
- **`RegionEvent::BlockEventsBegin` is said in a tick that has block events**, at the
  phase's start. The edge sends the changes collected before it there, and keeps the
  changes after it for the region's next tick, merged before that tick's own; it
  sends them also when the region is absorbed. A worker sends a link the next tick's
  delta, empty or not, after a delta whose changes ran past the mark, so that they
  always go. The mark alone is not sent.
- **The first sounds are those of a door, a trapdoor and a gate used by hand**, by the
  block set or the wood as the game's `BlockSetType` and `WoodType` give them, heard
  by all near but the player, the pitch drawn from the rules' stream. The rest of what
  phase B does is R6's.
- The comparison with the official server of what a client is sent (the order of
  changes and block events, a door's sound) waits for R5's pistons, the first block
  events a client can be compared by, and is R8's with the rest; the packets' layouts
  are SteelMC-26.3's (`c_sound.rs`, `c_block_event.rs`).

What redstone asks of a state (2026-10-17, before R3): the extract program asks the
game's `isRedstoneConductor`, `isSignalSource` and `hasAnalogOutputSignal` of every
state, kept in byte 11 of the table's rows (layout 3); a shulker box answers by its
block entity and the row holds the closed box's answer. The guess from shapes that wire
used before differed for 32 blocks (TNT, ice, glowstone, sea lanterns, copper bulbs and
the chorus flower do not conduct; spawners and closed shulker boxes do).

R3a, levers, buttons, wire, lamps, doors, trapdoors and gates (2026-10-17, by a
subagent, read against the jar's own classes, which 26.3 ships unobfuscated):

- **A change of block tells its neighbours of the block that was there**: the jar's
  `Level.setBlock` passes `updateNeighborsAt` the state `setBlockState` returned, the
  old one. R2b's note above, that a multiple update keeps "the block that changed as it
  was when the update was made", is to be read so: the block the change replaced.
  `set_block_updated` also runs the old block's `affectNeighborsAfterRemoval`, only
  where the block is another and the neighbours are told or a piston moved it, as
  `LevelChunk.setBlockState` does, then the new block's `onPlace`; it tells comparators
  of a block with an analog output; and it asks the old block's indirect shapes, then
  the neighbours', then the new block's, as `Level.setBlock` orders them.
- **Wire is the game's `calculateTargetStrength`**, not SteelMC's Lithium-shaped one,
  which skips the direction back; its `onPlace` updates up and then down, as the jar's
  `Direction.Plane.VERTICAL` is `{UP, DOWN}`, where SteelMC and Pumpkin go down first.
  The seven positions of its update are in `HashSet` order, ported from SteelMC with
  its fixtures and seen in the jar (the evaluator sets the wire with flag 2 only if it
  is still the state it measured).
- **A click on a lever, a button or wire runs its rule** (`Clicked::ByRule`); wire's
  dot and cross reads its neighbours, so wire joins the comparator in
  `use_reads_signal`. A wire whose use changes nothing passes, and the held item is
  placed, as the game's PASS does.
- **Placement reads power** (a lamp lit, a door, a trapdoor or a gate open and
  powered), and a placement whose state is worked out from a signal, wire's included,
  is refused where the target's chunk does not tick (`placing_reads_signal`).
- Wire set in a chunk that does not tick waits as a kept update of itself, and wire
  removed there tells all seven positions, each kept until its chunk ticks: a few more
  updates than the game makes, in a rare case.
- Sounds: a lever's click at 0.3, pitch 0.6 on and 0.5 off, heard by everyone; a
  button's click on by its block set, left out for the presser, its click off heard by
  everyone, released after 20 ticks (stone, polished blackstone) or 30 (wood); doors,
  trapdoors and gates moved by power heard by everyone, a door's and a trapdoor's
  before its set and a gate's after, as in the jar.

R3b, note blocks, bells, copper bulbs, rails, dripleaf, plates, tripwire, redstone ore
(2026-10-17, by a subagent, read against the jar):

- **The first typed block entity and the tickers are the bell's**
  (`region/block_entities.rs`), as section 5 says: tickers in the order made, those
  made during the phase waiting, a chunk's leaving at an unload and joining at the end,
  in position order, at a load, waiting where the chunk does not tick. The tick's
  phases are now block events, **entities**, block entities: the entity phase is where
  plates and tripwire look for the region's players.
- **Instruments are the jar's**: `clustine_blocks::note` with a table of the 796 blocks
  whose instrument is not the harp, read from the jar's `Blocks` (a block's own
  `instrument`, the block it copies, or its family's properties), not picked by hand.
- **A left click plays no note and lights no ore**: in creative the jar's
  `handleBlockBreakAction` breaks the block at once and never calls `attack`, and every
  Clustine player is in creative. Survival will need `attack`.
- Where SteelMC and the jar differ the jar holds: a bell under a ceiling needs the
  centre's support; a tripwire hook that is no longer a hook at its own position stops.
  With neighbours told of the block that was there (R3a), a redstone block placed
  beside a junction of three rails does not turn it, and a lever does, as in the game.
- **What a player's box presses is the box at the end of the tick's inputs**, not the
  box swept along a move, so a fast walk over a plate within one tick does not press
  it; nobody sneaks, as sneaking is not region state. A hook's line, up to 41 blocks,
  can reach beyond three by three chunks: where it meets a block the region cannot
  read, the hook does nothing.
- Not built: a player head's own note (its NBT is not read), the bell's raiders and
  resonance, shears on tripwire, game events and vibrations. A copper bulb has its
  analog output for R4's comparators.

## Review

**First review of this record (2026-10-17): ready after named changes**, which are
made:

1. Changes carried past the mark of block events could wait for ever for a next
   delta, or be lost with a link: a delta follows, and the edge sends what it carries
   at a link's end, an absorption or a later tick; sounds and the rest go in their
   order around the mark (section 8).
2. The order of tickers and kept events across loads, restores and merges
   contradicted itself: no counter; the running region's order in memory, rebuilt by
   chunk and position at a restore, a merge or a split (sections 4 and 5).
3. The game saves only a moving piston's earlier progress: a move through a reload
   takes a tick more, as the game's does (section 5).
4. A kept piston check would have run and been kept again every tick: a waiting
   piston, tried again only when the region's land changes (section 10).
5. Merging at once could not be built from what the coordinator hears and could flap
   with splits: moved to ADR-0027 (section 10).
6. No new seam kinds are needed, a kept entry is already one step with its source and
   flags; kept entries run in key order (sections 1 and 2).
7. Wire's update was misstated: `updateNeighborsAt` on each of seven positions
   (section 2).
8. Sounds had no fields: given, with their range, the player left out, a seed drawn
   by the edge, and a block placed or broken by another player (section 8).
9. A bell's ring is not durable (section 5). 10. Departures are worked out after the
   block entities (section 3). 11. The instant retraction's condition in full;
   quasi-connectivity reads three blocks (sections 1 and 9). 12. Clicks meet the
   predicate of the tick before (section 1). 13. What the plan said of uncoupled lines,
   the official server and `last_ticked` restored; what goes beyond the game named
   (sections 3, 8, 10 and 12). 14. Today's test of a seam entry stated as it is
   (section 1).

**Second, focused review (2026-10-17): accepted after changes**, which are made: a
waiting piston is tried again also when a chunk its move reaches starts ticking
(section 10); merging at once is ADR-0027's in the context and the consequences too,
and section 10 holds at every line; the context names section 10; an output-signal
update's direction is towards the block it came through (section 2); kept block events
are committed only when they change, and those put back go before those queued after
the phase (section 4).
