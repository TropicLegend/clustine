# Phase R: redstone, planned

Draft of 2026-10-17, revised the same day after two independent reviews (sections 8
and 9); to be put to the owner (section 6), before its record, ADR-0026, is
written from it. The phase the owner agreed on 2026-10-15 (B's question 4: "a phase of
its own right after B and the record between regions"), next in the roadmap's order
after phase E, toward vanilla parity (tier T4 of the [parity matrix](../parity-matrix.md)).

## 1. Why, and what a player meets

Today nothing is powered. Within seconds a player meets:

- a lever or a button that clicks into place and does nothing: no sound, no power;
- redstone dust that never lights, and a lamp beside a lever that stays dark;
- a door, a trapdoor or a fence gate beside a lever or a pressure plate that never
  opens (the roadmap's known limits say so);
- a torch on a block that a lever powers that never goes out; repeaters and
  comparators that pass nothing on; an observer that never pulses;
- a piston that never moves: no door of pistons, no farm, no flying machine, no slime
  launcher;
- a note block that plays nothing when it is powered, clicked or hit;
- redstone ore that does not light when clicked or stepped on;
- no sound for anything the server does: a door opened by power, a piston, and
  already from phase B a door opened by another player, water and lava, a block that
  breaks under an update.

Creative players build exactly these first, and the owner judges by playing.

## 2. Where things stand

From a read of the code and the reference port (2026-10-17):

- **Updates as phase B built them** (`clustine-sim/src/region/updates.rs`):
  `UPDATE_ORDER` is vanilla's (west, east, down, up, north, south), the shape order
  too; the collecting updater is depth first with vanilla's limits;
  `set_block_updated` runs `onPlace`, the neighbour updates and the shape updates in
  vanilla's order. Its entries are only `Shape` and the six-neighbour `Neighbours`:
  there is no single-target update with its source block (vanilla's
  `neighborChanged(pos, sourceBlock, orientation, movedByPiston)`), no update of the
  neighbours except one side, no `updateIndirectNeighbourShapes`, no
  `updateNeighbourForOutputSignal`. `neighbour_changed` only removes what cannot stay.
  Seam entries (`SeamKind`) and `Effected::Update` are stored formats.
- **Scheduled ticks** order as vanilla's (due, priority, sub-tick order). Missing:
  `hasScheduledTick` as a query, `willTickThisTick`, and the game's
  `isHandlingTick`.
- **The phases of a region's tick** (`region.rs:567-640`): inputs, seams, block ticks,
  fluid ticks, post-processing, random ticks. Vanilla's `ServerLevel.tick` runs block
  ticks, fluid ticks, the chunk tick, block events, entities, block entities; players'
  packets are handled outside the level's tick.
- **Block entities** are carried in chunks as a kind and NBT bytes. `Chunk::set`
  (`clustine-world/src/chunk.rs:316-334`) drops one when the kind changes, makes an
  empty one for a new kind, and keeps the data when the kind stays. The simulation
  never reads or writes one; a commit carries block states only, as do copies'
  updates (`CopyUpdate.changes`), replay (`worldstore/src/chunks.rs:1391-1452`, whose
  argument is that each change sets a definite state and the last wins), the worker's
  record of unsaved chunks, and the edge, which sends block entities only inside chunk
  packets.
- **No block-event packet, no block-entity-data packet, no sounds.**
- **A click** (`UseItemOn` → `RemoteStep::UseOn` → `Region::click_here` →
  `placement::use_block`) returns `Clicked::Used(Vec<(BlockPos, BlockState)>)`,
  applied all or nothing by `change_all` as plain sets (`region.rs:1559-1597`): doors,
  trapdoors, gates, flower pots. A lever, a button, a repeater, a comparator, a note
  block and wire take the click and do nothing. Placement (`placement::place`, the
  placing view `Rules::Placing`) reads no signal.
- **Only players are entities.** Falling blocks land at once.
- **Between regions** (ADR-0025): blocks across a line are read from copies; what a
  tick does across a line is an effect; while coupled an update is a tick late and a
  scheduled tick made by doing an effect counts from the maker's tick (E4), so it is
  on time. A chunk ticks where it and its eight neighbours are loaded or copied.

The reference for vanilla 26.3 is SteelMC-26.3 (`steel-core/src/behavior/blocks/
redstone/`, about 10,000 lines: wire, torches, diodes, levers, buttons, plates,
observers, pistons, lamps, note blocks, targets, bulbs, bells, rails, tripwire, doors
by power; missing TNT, dispensers, droppers, hoppers, crafters, sculk), checked
against the jar's classes wherever order matters: its wire evaluator is structured as
Lithium's, not as the game's code. 26.3's wire is the game's default evaluator; the
orientation-based one comes only with the Redstone Experiments pack.

## 3. The design proposed

### 3.1 Signals, as vanilla

The level's queries are ported (`world/signal_getter.rs`): `getSignal`,
`getDirectSignal`, `hasNeighborSignal`, `getBestNeighborSignal`,
`getControlInputSignal`, `isRedstoneConductor`, each answered by the block's own
signal rules, with wire's "no wire signals while wire measures" as a flag of the
query. They live beside the blocks' other rules (`clustine-blocks`), read through the
region's `block()`, so copies are read as loaded chunks are. Pure functions of the
blocks: the simulation stays deterministic.

**Nothing is read that is not there.** Redstone reads up to two blocks from the block
that runs (wire reads diagonals, a conductor's direct signal its own six neighbours,
quasi-connectivity the block above's neighbours, a comparator two blocks behind; the
piston's resolver reaches 13 blocks, within the same three by three chunks). One
predicate, **a chunk that ticks** as `update_ticking` defines it (`ticks.rs:255-266`),
decides where anything whose rule reads a signal runs: an update of such a block, a
seam entry or an effect run again (`run_seams_of`, `run_effect`, which use the six
chunks around the target today and take this test instead), a block event, a ticker,
and the rules a click or a placement runs. An update elsewhere is kept as a seam entry
until its chunk ticks, as an update into land not held is today (the updater already
turns one step of a chain into an entry and carries on, so the order inside one
region is the game's). A click or a placement whose rules would read a block that is
not there is refused, as one into land not loaded is today. So no rule reads a
missing block as air or as unpowered, and nothing settles dark beside power for good
when copies come and go.

### 3.2 Updates and ticks

- The updater gains vanilla's entries: a single target with its source block and
  whether a piston moved it, the six neighbours except one side, indirect shape
  updates, and the comparator's output-signal update; each block's
  `neighbour_changed` is given its source and the flag. New kinds of seam entry and of
  `Effected::Update` change the chunk's and the log's formats: R2 lists them and
  versions them.
- Wire is the game's default evaluator, checked against the jar's
  `DefaultRedstoneWireEvaluator` and `RedStoneWireBlock.calculateTargetStrength`: it
  sets itself with `UPDATE_CLIENTS` and updates the wire and its six neighbours in the
  iteration order of Java's `HashSet<BlockPos>` of 16 buckets (the position's hash
  `(z*31+y)*31+x`, spread as `HashMap` spreads it, `h ^ h >>> 16`, then `& 15`;
  insertion order within a bucket, inserted as the wire, down, up, north, south, west,
  east), as SteelMC reproduces it (`java_hash.rs`).
- Scheduled ticks gain `has_scheduled_tick(pos, block)`, false for a tick already
  drawn to run this tick, and `will_tick_this_tick(pos, block)`, true for a tick drawn
  and not yet run; each drawn tick leaves that set as it runs, and it is empty outside
  the phase of block ticks. Torches and diodes depend on the difference.
- **Handling a tick**: true from the start of the region's tick phases through the
  block events, the seam phase and effects included (an update that arrives as an
  effect was made while its maker handled its tick), false while inputs, entities and
  block entities run, as the game's `isHandlingTick`. A piston chooses its instant
  retraction by it, and by whether it ticked this tick (`last_ticked`, the region's
  tick number, not saved, as the game does not save it, and forgotten at a restore).
- Two phases end the tick, in vanilla's order: **block events** after the random
  ticks, then **block entities**.

### 3.3 Block events and block entities are kept in their chunks

- **The queue of block events** is in insertion order with exact duplicates dropped,
  and the phase runs it to empty, events queued while it runs included. An event
  whose chunk does not tick is put back, in order, for the next tick; one queued after
  the phase (by a moving piston that finishes, say) waits for the next. An event that
  outlives its tick is **kept in its chunk**, as the chunk's scheduled ticks are, with
  a number from the region's counter that orders it: committed with the chunk's ticks,
  carried by the chunk through merges, splits, returns and loads by another region,
  and dropped from the region's queue when the chunk is unloaded. The next phase takes
  the kept events of the chunks that tick, by their numbers, before those of the new
  tick. The counter is region state; a merge takes the larger.
- **Typed block entities**: the comparator's output, the moving piston (the moved
  state, its direction, extending or not, whether it is the source, its progress) and
  the bell (its ring and shaking; its search for raiders waits for entities), kept in
  the chunk beside the opaque ones and serialised into the chunk's NBT bytes as the
  game writes them. The others stay opaque.
- **The tickers** (moving pistons, bells) run in the game's list order: in the order
  they were made, by the region's counter; those made during the phase wait for the
  next tick; a chunk's tickers leave the list when it is unloaded and join it at the
  end when it is loaded, as the game drops them at an unload and adds them at a load.
  A ticker in a chunk that does not tick waits. The moving piston ticks three times:
  made in the block-event phase and ticked the same tick (progress 0 to 0.5), the next
  (0.5 to 1), and the next, which finishes it, each piston at its own position; the
  record states it against the jar. Its progress is in the chunk, so a move between
  workers, a crash, a merge or a split carries a move half way with nothing more.
- **One ordered list of changes in a commit**: a commit's `changes` become
  `Block(position, state)` or `BlockEntity(position, data)`, in the order they
  happened, and replay applies them in order as the chunk's own `set` does. A set of
  a block of the same kind that the game would make a fresh block entity for writes
  the empty block entity explicitly. Replay stays idempotent: each change sets a
  definite state and the last wins (the store's crash tests include a block entity's
  kind changed across a checkpoint). The same list goes into copies' updates (a change
  of a block entity alone advances a copy's chain), the worker's unsaved chunks, the
  store's `put`, and the edge's replica, whose chunk packets carry a moving piston's
  data.
- The torch's burnout list, which keys on the region's tick numbers, is dropped at a
  reshape and a restore, as a restart of the game drops it.

### 3.4 Clicks and placement run the blocks' rules

- **A click runs the block's own `use` rule inside the updater**, in place of a list
  of states set all or nothing: the lever's update of its support's neighbours, the
  button's release tick, the comparator's mode and output, the repeater's delay, the
  note block's note and block event, wire's dot and cross, redstone ore lit, doors
  and the rest as now. A click refused (a chunk not loaded, land not held) changes
  nothing, as now. A left click on a note block plays it; stepping on redstone ore
  lights it.
- **Placement runs `onPlace` and `setPlacedBy` and reads signals**: a lamp, a door, a
  trapdoor or a rail placed beside power comes out powered; a piston placed powered
  extends; a repeater beside power schedules its tick.
- Inputs run outside "handling a tick" (3.2).

### 3.5 Block events, sounds and the client

- `RegionEvent::BlockEvent { position, block, kind, data }`, sent to viewers as the
  game's block-event packet (a piston's move, a note, a bell).
- **Order on the wire as vanilla's**: the game broadcasts a tick's block changes in
  its chunk tick, before its block events, and the changes made from the block-event
  phase on go in the next tick's broadcast, coalesced per position with that tick's;
  the client re-runs each move from its block event on its own world. A region's
  events carry one mark a tick, "block events begin": the edge sends the changes
  before it, then the block events, and carries the changes after it into the next
  tick's batch, which it already coalesces per position. Flushing before every event
  would show a client's own pistons a world the game's client never sees. Block events
  go to players within 64 blocks, as the game sends them. Checked against the
  official server.
- `RegionEvent::Sound` and the level events (a torch burning out smokes and hisses):
  who hears a sound is per block, as the jar says: a lever's click is played to
  everyone, the clicking player too; a button's, a door's, a comparator's leave out
  the player whose own client plays it. Every block is checked against the jar.
- The sounds of what phase B does (a door opened by hand, heard by others; water and
  lava; a block that breaks under an update) come with them, a step of their own.
- Whether the client needs block-entity data for the moving piston, or builds it from
  the block event and the block change alone, is settled against the official server
  in the step of pistons.

### 3.6 Pistons and players

Players are entities now, and a creative player stands in front of a piston within
minutes. The client owns its player's position (`play.rs:254-283` checks only for
numbers that are not numbers and for an unconfirmed teleport; the region takes the
position, `region.rs:1381`): it pushes itself by its own moving piston and is
launched by slime on its own. So the region's push of a player decides only where it
holds the player between the client's packets, which other players see, and it never
sends that player's own client a correction or a motion, which would pull them back.
A push that carries a player over a region's line is a crossing as any walk over it.
Other entities wait for their phase.

### 3.7 What is built, and what waits for entities

In this phase:

- **Sources**: lever, buttons pressed by hand (with their release tick), redstone
  block, redstone torch (with burnout), pressure plates and tripwire pressed by
  players, redstone ore.
- **Wire**, with its shapes, its dot and cross by a click.
- **Diodes**: repeater (delay by click, locking), comparator (mode by click; the
  analog outputs of blocks without an inventory: cake, composter, cauldrons, end
  portal frame, jukebox's state, respawn anchor, beehive's level; containers wait).
- **Observers**, **target blocks** (powered by signals; arrows wait).
- **Consumers**: lamp, doors, trapdoors, fence gates, note blocks, copper bulb,
  powered and activator rails, a rail's shape by power, big dripleaf, bells.
- **Pistons and sticky pistons**: the structure resolver with its limit of 12, slime
  and honey, push reactions, quasi-connectivity, block events, the moving piston,
  instant retraction, players pushed.

Waiting for the phase of entities (item entities, inventories, projectiles, mobs):
TNT, dispensers, droppers, hoppers, crafters, sculk sensors and shriekers, detector
rails, arrows on buttons and targets, plates and tripwire pressed by mobs and items,
pistons pushing other entities, comparators reading containers. The daylight
detector waits for light in the simulation (ADR-0024, section 8).

### 3.8 At a region's border

- **Timing across a coupled line**: repeaters, torches, comparators and observers
  keep their timing (a scheduled tick made by an effect counts from the maker's
  tick); wire, lamps turning on, doors, rails and pistons are a tick late per
  crossing, as an update is. Two paths into one gate that cross the line differently
  arrive skewed, and a skew can make a pulse the game would not: a copper bulb or a
  flip-flop toggled wrongly, a repeater's lock (set by a shape update, itself a tick
  late) holding the wrong bit, an observer on fading dust pulsing several times.
  Settled results can differ, not only timing. Uncoupled, given up or preparing, an
  effect is a tick or a few late, and more can differ. Question 2.
- **Dust across a line goes dark slowly**: the default evaluator turns a line of dust
  off within a tick by a cascade; across a line each step waits for an effect, so the
  dust falls about a level a tick, up to 15 levels, 0.75 s to 1.5 s, and longer while
  uncoupled. It cannot oscillate: with the source gone every level only falls.
- **A piston moves only within land it holds**: every position the move sets (where
  the blocks go, those it destroys, the head, and the moving pistons an instant
  retraction finishes) and every block it moves lies in a chunk that ticks and that
  the region holds (a region has a chunk loaded only while it holds it), or the piston
  does not move. The game checks whether to move only when it is updated or placed,
  so a piston refused for its land would stay put while powered for good; its check is
  **kept as a seam entry** that runs again when its land is held, as an update into
  land not held is. A push is never turned into conditional sets into another
  region's land. At a pinned line that is never; along a line between regions that
  follow players it is once one region holds both sides, which depends on where the
  players are. Question 3.
- **Plates and tripwire see the region's own players**: a player of the other region
  standing over the line on a plate does not press it.
- Block events and moving pistons are the region's own; viewers across the line see
  them through their own edge.

### 3.9 Durable state

- Block changes and block-entity changes are committed every tick they happen, in
  one ordered list.
- Block events that outlive their tick and the tickers are kept in their chunks, the
  region's counter in its state (3.3).
- Scheduled ticks of redstone are chunk ticks as any (ADR-0024).
- The torch's burnout list is not durable and is dropped at a reshape (3.3).

## 4. Steps

| Step | What | Who | Judged by |
|---|---|---|---|
| R1 | ADR-0026 from this plan, reviewed | me | an independent review |
| R2a | The store: the ordered list of block and block-entity changes in commits, the log, replay, copies' updates, `put`, chunk saves; the formats of new seam entries and updates, versioned | me | the store's crash tests at every point, extended |
| R2b | The simulation's ground: the updater's new entries with source and flag; `has_scheduled_tick`, `will_tick_this_tick`, handling a tick; the phases of block events and block entities; events and tickers kept in chunks, through unloads, merges, splits and restores; typed block entities; clicks run inside the updater; placement's hooks; 3.1's predicate for everything that reads a signal; a piston's kept check | me | simulation tests; the reshape tests extended |
| R2c | The edge: block events, sounds and level events with their packets; the mark of a tick's block events and the changes after it carried to the next tick; block-entity changes in the replica | me | the official server |
| R3 | Signal queries; sources, wire, consumers (3.7's first, second and fifth lists) | a subagent, in `clustine-blocks`, the simulation's redstone module and placement's hooks | simulation tests from the game's numbers; the placing oracle |
| R4 | Torches, repeaters, comparators, observers, targets | a subagent, after R3 | as R3, with timing tests (a clock of repeaters, a torch's burnout) |
| R5 | Pistons, with players pushed and launched | me, after R4, since flying machines need observers (block events, block entities and reshapes are where ordering hides) | structure tests (12 and 13, slime, honey); the official server for the moving piston and a player pushed |
| R6 | The sounds of phase B | a subagent, after R5, so that no two work in the simulation at once | the official server |
| R7 | Tests from the record by someone who did not write the code | a subagent | — |
| R7b | ADR-0027, lending at a pinned line, and merging at once with redstone at a line, reviewed and built | me (the store's table and hand-overs) | the store's crash tests; a circuit across a pinned line against the same circuit in one region |
| R8 | Comparisons with the official server, timing and settled results; measured commits of a busy circuit; what to try with a real client, in the roadmap | me | — |

## 5. What the oracle can and cannot judge

The placing oracle compares the blocks a second bot is sent after a script: it suits a
settled circuit (a lamp lit, a door open, a piston extended, dust's levels). The
timing of a circuit (a repeater's delay, a clock's period, a piston's ticks) is judged
by simulation tests against the game's numbers and by a bot that notes the tick each
block change and block event arrives in, on the official server and on Clustine, in
one region; also the order of block changes and block events on the wire (3.5), and a
contraption that depends on wire's hash order.

## 6. Questions for the owner

1. **What waits for entities**: TNT, dispensers, droppers, hoppers, crafters, sculk,
   detector rails, arrows on buttons and targets, plates and tripwire pressed by
   anything but players, pistons pushing anything but players, and comparators
   reading containers; and the daylight detector, which waits for light in the
   simulation. Players are pushed by pistons in this phase. Recommended: yes,
   with the phase of entities, which comes next.
2. **Redstone across a line between regions whose border ticks**: repeaters, torches,
   comparators and observers keep their timing; wire, lamps, doors, rails and pistons
   are a tick late per crossing, dust across a line goes dark over 0.75 to 1.5 s, and
   a circuit whose paths cross the line differently can end otherwise than in the
   game (a flip-flop toggled wrongly, a repeater's lock holding the wrong bit). Only
   at a pinned border, or along a line between regions that follow players where
   both sides tick. Making it exact needs both regions in one tick, which ADR-0003
   rejects. Recommended: accept, and name it in the record and the roadmap; a circuit
   built across a pinned border is the case to avoid.
3. **A piston moves only within its region's land**: at a pinned border a piston
   whose push or pull would cross never moves; between regions that follow players
   it moves once one region holds both sides (its check is kept and runs again then),
   which depends on where players are.
   The alternative, carrying a move across as conditional sets of the blocks moved,
   makes the move's two ticks a matter of two regions and is a record of its own.
   Recommended: the rule, named, and the alternative left for later.

**Answered on 2026-10-17.** (1) As recommended: what needs entities waits for their
phase. (2) "Look for more", and (3) "carry moves across": the ways are in
[r-lines.md](r-lines.md), reviewed; the owner chose there, the same day, **lending at
build time** at a pinned line (a component or a piston placed within reach of the line
borrows the chunks across it that it reaches, while it stays), and merging at once two
regions that follow players with redstone active at their line. So 3.8 holds only
until the lending is built: a record of its own, ADR-0027, after ADR-0026 and before
redstone at a pinned line is called done. The steps of section 4 inside one region go
ahead meanwhile.

Decided by the plan and not asked, under the owner's rule against deferring what a
player notices within minutes: wire is the game's default evaluator with its hash
order; sounds come with this phase, those of phase B too; pistons push players.

## 7. Risks

| Risk | What is done |
|---|---|
| Java's `HashSet` order reproduced wrongly | Ported with SteelMC's tests, checked against the jar's classes, and compared on the official server by a contraption that depends on it |
| The ordered list of changes touches the log, replay, copies and checkpoints | R2a by me, with the store's crash tests at every point |
| Block events and tickers kept in chunks through unloads, merges, splits and crashes | R2b by me; reshape and crash tests of a piston mid-move, an event deferred, and a chunk unloaded with both |
| A busy circuit makes commits and copies' updates grow | Measured in R8: a clock of repeaters and a line of 64 dust, their commits a second |
| The client needs more for the moving piston, or another packet order | Settled against the official server in R2c and R5 |
| A player pushed by a piston is pulled back by something the server sends | No correction or motion to the pushed player's client; a bot pushed on the official server and on Clustine in R5 |

## 8. The first review (2026-10-17), and what changed

1. Block events outlive a tick (an event in a chunk that does not tick is put back;
   one queued after the phase waits): the queue is region state, carried and durable
   (3.3).
2. Redstone reads two blocks out, and a missing block read as unpowered could settle
   dust dark for good: redstone runs only where the chunk and the eight around it are
   there, else waits as a seam entry (3.1).
3. Clicks set lists of states and cannot run a block's rule: a click runs inside the
   updater, owned by R2b (3.4).
4. Placement reads no signal: placement's hooks, in R2b and R3 (3.4).
5. Block-entity changes against block changes were unordered: one ordered list, with
   fresh block entities written explicitly (3.3).
6. A move could span a reshape's line: a reshape finishes moves (3.3).
7. "Does not move" was undefined against the land: every position set and every
   block moved in land the region holds (3.8).
8. The game's `isHandlingTick` was missing (3.2).
9. `will_tick_this_tick` was described wrongly (3.2).
10. The tickers' order was unspecified: vanilla's list order, region state (3.3).
11. The moving piston ticks three times (3.3).
12. A lever's click is played to everyone; who hears is per block (3.5).
13. The torch's burnout list keys on tick numbers: dropped at a reshape (3.3).
14. SteelMC's wire evaluator is Lithium's structure: checked against the jar (3.2).
15. Block changes and block events reach the client in the game's order (3.5).
16. Across a line, skewed paths can change settled results, not only timing: named,
    and question 2 rewritten (3.8).
17. Plates at a line see only the region's players (3.8).
18. Pistons push players, who exist now (3.6).
19. Note blocks played by a hit, a torch's burnout smoke, redstone ore lit (3.4, 3.5).
20. R2 split in three; R5 not delegated, after R3; the sounds of phase B a step of
    their own; formats of new updates listed (4).
21. The question of wire's order was no choice and is decided; question 3 names the
    pinned case and the alternative; pistons and players decided, not asked (6).

## 9. The second review (2026-10-17), and what changed

It found the first review's answers right but for these, and checked the rest
against the reference: block events run to empty, those queued while they run
included; the span of handling a tick; tickers made in the block-event phase tick the
same tick; the moving piston's three ticks; every read of redstone within three by
three chunks; an update kept mid-chain without changing the order in one region; the
ordered list idempotent at replay, fairly but not fully sure.

1. A piston refused for its land was never tried again: its check is kept as a seam
   entry (3.8), and question 3 says so.
2. The rule of what may be read covered updates only: one predicate for everything
   that reads a signal, clicks and placements refused where it fails, and the seam
   retry and effects using it (3.1).
3. Flushing block changes before every block event was not the game's order: one
   mark a tick, and the changes after it carried into the next tick (3.5).
4. Tickers and deferred events in chunks that are unloaded or given away were
   undefined: both are kept in their chunks, as scheduled ticks are (3.3).
5. Finishing moves at a reshape was needless and itself unordered: dropped; a move's
   progress is in its chunk, and an instant retraction's reach is part of the land a
   piston must hold (3.3, 3.8).
6. `last_ticked` named, the region's tick, not saved (3.2).
7. The bell needs a typed block entity (3.3).
8. The edge checks no movement: the client owns its position, and the push never
   corrects the pushed player (3.6).
9. One predicate for where things tick, `update_ticking`'s (3.1).
10. Handling a tick holds for the seam phase and effects (3.2).
11. R6 after R5, so that no two subagents work in the simulation at once; R5 after R4
    (4).
12. The hash is spread as `HashMap` spreads it (3.2).
13. Block events go to players within 64 blocks (3.5).
14. The edge's replica takes block-entity changes (3.3).
15. The daylight detector is named in question 1 (6).
