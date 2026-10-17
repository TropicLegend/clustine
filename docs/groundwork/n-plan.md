# Phase N: entities other than players, planned

Draft of 2026-10-17, revised the same day after two independent reviews (sections 8
and 9), and agreed with the owner's answers (section 6); its record, ADR-0028, is
written from it. The third item of the order agreed on 2026-10-15 (the roadmap, "Next"):
entities other than players, "a record of how entities cross regions and survive
merges, splits and crashes comes first", toward tier T3 of the
[parity matrix](../parity-matrix.md), and what redstone has waiting for entities
(ADR-0026, section 11).

Marks: a `file:line` is something read there; a class and method of the game is read in
the 26.3 jar with `javap`. **Inferred** is reasoning from what was read, to be tested.
**Guess** is from memory of the game and checked against nothing here.

## 1. Why, and what a player meets

Today the only entities are players. Within minutes a creative player meets:

- a spawn egg that does nothing: no animal, no monster, anywhere;
- a world without life: generation drops the animals a chunk is made with, and the
  structures drop their villagers, illagers, guardians and the rest
  (`clustine-features/src/structures/*`, s-plan section 5);
- sand and gravel that land at once instead of falling, and a falling block that
  cannot land that is lost (`region/ticks.rs:557-650`);
- nothing that drops: a torch washed away, leaves that decay, a block broken by an
  update, a piston's moved block that breaks, all leave no item; an item thrown with Q
  goes nowhere;
- TNT that does not light or explode, and dispensers, droppers, hoppers, detector
  rails and the other parts of ADR-0026 section 11 that wait for entities;
- no boats, minecarts, armour stands, item frames or paintings (the last two are
  entities in the game).

## 2. Where things stand

- **Players** are `EntityKind::Player` (`clustine-sim/src/api.rs:227`), kept in the
  region's state (`state.rs:17`, `56`), with a pose and no velocity, and no
  server-side movement physics: `move_to` only clamps (`region.rs:2464-2495`).
- **Ids**: `EntityIds`, blocks of 2^20, issued by the store to pinned and home regions
  only (`worldstore/src/lanes.rs:2365-2392`); a split's part gets the empty block
  (`reshape.rs:90-96`); the home region issues every player's id, and the higher id is
  the later stay (ADR-0010, 0014, 0020). ADR-0010:318: "when things other than players
  need ids away from home, that has to be decided". `Assignment::entity_ids` is filled
  and read by nothing.
- **Crossing** is the players' own: `Durable::Departed` through the edge's outbox and
  `PlayerArrive` at the other region, ordered by `(entity, hops)`; a merge keeps the
  later stay; a split takes the players on the part's side.
- **Durability**: stays in commits (`StayNote`, `LoggedStay`), the player's record in
  the store; region state as opaque bytes in each commit and in checkpoints.
- **Chunks** carry no entities (ADR-0022:126-131; world-format.md:401 "Not stored
  yet: Entities"). Block entities are block data and stay so.
- **Between regions**: effects (ADR-0025) are addressed to blocks, carried by the
  store's per-region stream with sequences, durable in commits, dropped into chunks
  nobody uses; border copies of the 3x3 around viewer and guest chunks, kept current
  by commits' block changes; coupling makes a border tick in lockstep. Lending
  (ADR-0027) moves chunks across a pinned line where a circuit works.
- **Edge**: shows an entity whose chunk is in the viewer's view (no tracking range),
  accepts moves only from the region that introduced it (`fanout.rs:~2697`); sends
  `SpawnEntity` with zero velocity and absolute `SyncEntityPosition` every move. Open
  since ADR-0006: with several edges, two regions' reports can arrive out of order, so
  entity states need versions.
- **Protocol**: spawn, sync position, head rotation, remove; ids but no structs for
  entity data, motion, equipment, relative moves, events, damage, interact
  (`protocol/src/generated/packet_ids.rs`).
- **Data from the jar**: only entity type names and ids, and entity type tags. The
  game's data generator output (`target/datagen/26.3/generated/`) has entity loot
  tables (94), block loot tables, and the biomes' spawn lists
  (`attributes["minecraft:gameplay/natural_mob_spawns"]`). Dimensions, eye heights,
  categories, tracking ranges, default attributes and spawn placements are in code
  only and reachable from `Extract.java` as its other tables are.
- **Oracle**: the official server harness kills all entities and turns mob spawning
  off (`botswarm/src/oracle.rs`); bots record entities' absolute positions only.
- The game is **creative only**: there is no survival mode, health, hunger or
  inventory beyond the hotbar.


## 3. The design proposed

### 3.1 Identity: a durable key, the game's id and the wire's id

An entity has a **key**, a `u64` unique for the life of the world, and a UUID made from
it (below), saved with it. Keys come in **blocks of 2^32** from the store: a region is
given a fresh block at **every hello** (written to its region file, as `EntityIds` are
now, `lanes.rs:2363-2392`), and when it is half way through its block it asks for the
next, given in a commit group. A split's part is given its block in the split's group,
so that its first item does not wait for the store. A fresh block at each hello means
a key given out in ticks a crash lost is never given again to another entity (the
first review, D1), and the UUID with it. Stays keep their order: the home region's
block is above every stay's key, as `lanes.rs:2367-2378` keeps it now.

The **game's id** (`Entity.getId()`) is game logic, not only the wire's: an item at rest
moves on the ticks where `(tickCount + getId()) % 4 == 0` (`ItemEntity.tick`), and a
mob re-evaluates its goals by `tickCount + getId()` (`Mob.serverAiStep`). The sim's
`id()` is the key's low 32 bits, used wherever the game uses `getId()`.

The **wire's id** (the protocol's `i32`) is the edge's: each edge maps keys to ids of its
own, given out in order. A player's own id stays mapped while they are connected;
another id is freed when no connection of that edge shows its entity, a connection
being ordered so that a client takes the removal before any reuse. The edge translates
**every** id in a packet, not only the entity's own: clientbound `SetPassengers`,
`SetEntityLink`, `TakeItemEntity`, `DamageEvent`, the owner in `SpawnEntity`'s data, and
entity data fields that hold ids (a fishing hook's hooked entity, a firework's
attached entity, a guardian's target); serverbound interact and pick-item-from-entity
the other way. An id the edge does not know is passed over. With several edges two
clients may know one entity by two ids, which no client can tell.

### 3.2 Where an entity lives, and in what order it ticks

An entity belongs to the chunk its position is in and lives in the region that holds
that chunk. Its state is part of the **chunk** (`clustine-world`'s `Chunk`, amending
ADR-0022:124-131), not of the region's opaque state, because the store replays chunks'
changes at a restore (`messages.rs:743-745`) and builds copies' bases from commits
(`chunks.rs:995-1030`): both need to read entities by chunk.

- **Saved apart from the blocks**: a chunk's entities are a record of their own in the
  store, beside its blocks, as the game keeps `entities/` files apart from the region
  files (`EntityStorage`). A mob's step then does not mark the blocks dirty, nor grow a
  merge's or a split's pause of chunk saves; and a later import of a game world (W4)
  reads them from where the game puts them.
- **With the chunk**: merges take the absorbed region's entities with its chunks;
  splits give a part those of its chunks; a lend lends a chunk with its entities; a
  return takes them with it as at its last commit. Nothing is decided per entity.
- **A chunk no region holds** keeps its entities frozen in the store, as the game keeps
  them in a chunk that is not loaded.

**Ticking.** In the game entities tick where `DistanceManager.inEntityTickingRange`
holds, one ring inside the chunks that take random ticks (`ServerLevel.tick`,
`forEachBlockTickingChunk`). Here: an entity ticks where its chunk's **5x5** is loaded or
copied in its region, and is frozen elsewhere; ticking is the same function of the
state across merges and splits.

**Order in the tick** (adding to ADR-0024 section 6a, as `ServerLevel.tick` runs):
block ticks, fluid ticks, spawning and random ticks, block events, **then entities**,
then block entities. Entities tick in ascending key; a passenger ticks with its root
vehicle (`Entity.rideTick`). Lookups such as `getEntities` in a box go by section and
then by key, so that what an entity finds is a function of the state.

### 3.3 Durable state: every tick's entity changes in the commit

The tick is deterministic, but the log records results, not inputs, so an entity's
state must be in the commits or a crash loses it. Each commit carries the **entity
changes** of its tick, keyed by chunk and key: an entity **whole** (spawned, arrived, or
moved into the chunk from another), **removed** (died, picked up, sent, moved out to
another chunk), or **fields set** (only what changed). A move between two chunks of a
region is a removal in one and the entity whole in the other, so that a copy of the
second chunk is never sent fields of an entity it does not have. Replay sets fields to
definite values, so it is idempotent as block replay is. Entity changes count as the
chunk's changes for a copy's gap check (ADR-0025 section 3).

What changes in a tick is more than position (the second review, D3): a mob's goal
timers, memories and head and body rotations change most ticks. To keep at rest what
is at rest:

- **counters are derived**: a birth tick rather than an age, a "until tick" rather than
  a countdown, where the game counts down by one a tick;
- **randomness has no state**: an entity's draws come from a stream counted from (world
  seed, key, tick, draw), not a generator that advances. The game's own entity randoms
  are seeded from the clock and cannot be followed anyway (section 5);
- the rest is measured in N2 (items) and N7 (mobs with AI), as R8 measured circuits.

**Copies** gain entities by the same route: a copy's base carries its chunk's entities,
its updates the entity changes of later commits; a region reads the entities across its
border at most one tick late while coupled.

### 3.4 Crossing a line: arrivals

An entity whose position comes into a chunk that its region does not have loaded and
ticking (another region's; its own but not loaded, or asked for and not yet loaded; a
chunk it is lending) is **sent** in the tick that moved it: removed in that commit and,
in the same group, written as an **arrival** addressed to the chunk, filed by the store
as effects are (ADR-0025 section 2). Commit and arrival are one group, so the entity is
in exactly one place in every durable state.

- The holder takes an arrival before its entities tick in the tick that takes its
  effects; an arrival is ready once its chunk is loaded (it needs no copies of the 3x3)
  and is never deduplicated. The taking commit carries the entity **whole**, so that
  the store may drop the arrival and later collect the sender's record.
- While the border is coupled (ADR-0025 section 8) the holder runs border time t + 1
  only once the sender is durable through t, so the entity is not delayed at all.
  Uncoupled, it pauses one to a few ticks, which a client interpolates over.
- **A chunk held but not used**, or held by a region in a handshake, in `Preparing` or
  being moved, keeps the arrival filed and written forward, as an effect not yet taken
  is, until it is used again; it is never dropped, at `Dropped`, `Unreadable` or
  `Withdrawn` either (ADR-0025 section 5 drops effects there; arrivals differ).
- **A chunk no region holds** (in the store's table) takes the arrival into its stored
  entities: on the thread for chunks, behind any saves of that chunk still pending, and
  only once the group is durable. Never while a region holds the chunk: a holder's
  restore replays its commits over the stored chunk and would undo the write (the first
  review, D1).
- A lend while an arrival is in flight: at the grant the chunk's use passes with what is
  filed for it, as for effects (ADR-0027 step 4).
- An entity's state can be larger than a log record (`MAX_RECORD_LENGTH`): a chest
  minecart, a shulker box item. It is split over records of the group, as a chunk is.

A **player's** crossing stays as it is, through the edge, which owns the connection.

**Vehicles and riders.** The root vehicle and its passengers cross as one: one arrival
where no player rides; inside the player's hand-over (`PlayerTransfer`) where one does,
with the vehicle and its other passengers carried with it and taken in by the new
region with the player. A second player in the same vehicle crosses by their own
hand-over, ordered after the first (to be designed in the record: N3). Links between
entities (a leash, an owner, a target, a hanging entity's block) refer to keys and
blocks across the line through the copies.

**The edge** shows an entity by its key from the region that introduced it, and by
`hops`, raised at every change of holder (a crossing, and at a merge, a split or a lend
for every entity whose holder changes):

- a report with higher `hops` takes over; one with lower `hops` is passed over, and so
  is a removal: the sender reports "handed on at `hops` h", never a removal, so that a
  late report from the old holder cannot remove what the new holder shows;
- a move from the new holder continues the old holder's moves on the wire: no removal,
  no new spawn;
- an entity handed into a chunk outside the edge's view is removed from it.

This is the version ADR-0006 left open, and it orders the reports of two regions that
reach one edge in either order.

### 3.5 Interactions across a line

What one entity does to another across a line goes as an **entity effect**, addressed
to the chunk the sender's copy shows the entity in and to its key, carried as block
effects are and done by the holder's rules one tick late (or on the game's tick while
coupled). The holder looks the key up in its whole table and passes the effect over if
the entity is not there, as the game's would miss:

- a hit (a player's attack, a mob's, an arrow's), with its damage source and knockback;
- a push between entities (`Entity.push`);
- an explosion's damage and knockback on entities across the line, and its blocks as
  block effects (`SetIf`).

**Pick-up** happens only in the player's own region's land: a player picks up the items
in their box that their region holds, and an item across the line once it has crossed
or the player has. In creative, `Inventory.add` never fails: what does not fit is
destroyed (`hasInfiniteMaterials`), so a pick-up needs no way back. What an entity
**sees** across a line (a target, a player to look at, a flock) it reads from the copies.

Where regions follow players they **merge** when entities work at their line, as
circuits do (ADR-0027 section 7): a mob chasing a player across it, a stream of items,
a crowd. Pinned lines use arrivals and effects throughout.

### 3.6 Simulation

Each entity type is ported from the game's classes, as blocks were, with the tables it
needs extracted from the jar by `Extract.java` (dimensions and eye heights, category,
tracking range and update interval, default attributes, spawn placements, synched data
defaults), and loot tables and spawn lists read from the generated data.

- **Movement and collision**: `Entity.move` against the blocks' collision shapes, with
  step-up, gravity and drag per type, fluids (pushed by flowing water, floating, lava),
  on-ground and fall distance. `f32` where the game uses `float` (drag `0.98F`, friction,
  `0.91F`, rotations), widened as Java widens, and no fused multiply-add. `Mth.sin` and
  `Mth.cos` are the game's 65,536-entry float table (`Mth.SIN`). The game uses HotSpot's
  `Math.sin` and `Math.cos` in three places entities meet: the Q throw
  (`LivingEntity.createItemStackToDrop`), elytra flight (`updateFallFlyingMovement`) and
  TNT's priming: ported with Java's `StrictMath`, a named exception as the icebergs' is
  (parity matrix, T2).
- **Pistons** push entities (today they leave players to push themselves,
  `pistons.rs:18`).
- **AI**: the goal selector, the brain where a mob has one, move, look and jump controls,
  and path finding with the game's node evaluators.

### 3.7 Spawning

**With the chunk.** `NaturalSpawner.spawnMobsForChunkGeneration` draws from a
`WorldgenRandom` with `setDecorationSeed(worldSeed, x, z)`: which animals a chunk is made
with, how many and where are a function of the seed, as features are, and judged
exactly with them (the features judge). Their colours and variants come from the
entity's own random and are judged by bounds. `Animal.checkAnimalSpawnRules` needs a raw
brightness above 8, so this needs light where generation runs (question 4).

**Natural spawning.** The game checks two caps: a world-wide one
(`SpawnState.canSpawnForCategoryGlobal`, the category's maximum times the spawnable
chunks over 289, against the whole level's count), and per player
(`LocalMobCapCalculator`, by the players within 128 blocks of a chunk). Creatures try
only every 400 ticks. Spawning reaches eight chunks from a player, copies reach one: so
the store publishes, for each chunk, the mobs in it by category, and the world's
spawnable chunk count and totals, and a region reads them for the chunks around its
players and for the world-wide cap, a few ticks late. Monsters spawn by light (question
4).

### 3.8 The edge

Per-type tracking range and update interval from the jar, as `ChunkMap.TrackedEntity`
does (within the viewer's view, as now); entity data (`SetEntityData`), motion, relative
moves and rotation with the game's periodic absolute sync, equipment, events
(`EntityEvent`), hurt, animations, pick-up (`TakeItemEntity`), passengers and links.
Serverbound interact (attack, use on entity), and a ridden boat's or horse's movement
from the client (`MoveVehicle`).

## 4. Steps

Each step a verified commit. N1 and N3 are not delegated (the edge's hand-over and
resume, and ordering); tests for N1 to N3 are written from the record by someone who
did not build them.

- **N0** The record, ADR-0028, from this plan and the owner's answers; reviewed.
- **N1 Keys and ids.** `u64` keys in blocks from the store (fresh at each hello, a
  split's in its group); the sim's `id()`; the edge's map of keys to wire ids, with
  every id in a packet translated; players carried over.
- **N2 Entities in chunks, durable.** A generic entity state in the chunk, its store
  record apart from the blocks, entity changes in commits and checkpoints, restore,
  merges, splits, lends, returns, copies; the edge showing a generic entity by key and
  `hops`. The first entity is the **item**, at rest, thrown by a creative player's Q or
  dropped from the creative tab (`SetCreativeModeSlot` with slot -1), with the thrower
  and the pick-up delay; the player's whole inventory (36 slots, the off hand and
  armour) so that what is picked up has somewhere to go. Measured as R8 was, items
  crossing a pinned line back and forth among them.
- **N3 Crossing.** Arrivals through the store, into land held, unused or held by
  nobody; entity effects; vehicles crossing with riders; the edge's continuity by
  `hops`; merging where entities work at a line.
- **N4 Physics, items and falling blocks.** `Entity.move` and collision, the item's tick
  (merging, despawning after five minutes, pushed by water), block loot tables (with
  their conditions: a block broken by a rule drops as without a tool; a creative
  player's break drops nothing) and drops from blocks broken by rules (water, decay,
  updates, pistons), falling blocks as entities, pick-up, pistons pushing entities.
- **N5 Vehicles and things a creative player places**: minecarts on rails, boats driven
  by the client, riding, armour stands, item frames, paintings and the other hanging
  entities, leashes.
- **N6 TNT and explosions**, and the redstone of ADR-0026 section 11 that waits for
  entities (dispensers, droppers, hoppers, detector rails, pressure plates that count
  entities, tripwire).
- **N7 Living entities and animals**: health, attributes, damage, death and loot,
  knockback; players hitting mobs (interact); spawn eggs; goals, controls and path
  finding; the animals, breeding and ageing; animals made with the chunk; the
  structures' entities placed at generation (villagers, illagers, guardians, …) with
  their simple goals, the villagers' brain left to N10. Measured: the log and the
  tick's length with many mobs.
- **N8 Light in the simulation** (the roadmap's next item, brought here; question 4).
- **N9 Monsters**, projectiles (arrows, snowballs, eggs, tridents), burning in daylight,
  natural spawning with both caps, difficulty.
- **N10 Villagers' brain and trading**, golems, raids, as a phase of their own
  (question 3).

## 5. What the oracle can and cannot judge

The official server seeds every entity's random and the level's random from the clock
(`RandomSource.create()` in `Entity.<init>`), so it does not repeat itself from one run
to the next; the order in which it ticks entities is deterministic (insertion order, and
sorted sections). So:

- **Exactly**: what draws nothing once it starts. A `/summon` with a fixed motion of a
  falling block, an item, a minecart and primed TNT's flight and fuse; an entity pushed
  by water; the animals a chunk is made with (section 3.7). The official side runs
  `/tick freeze`, then `/tick step 1` and `/data get entity … Pos Motion` each tick from
  the console, and Clustine's side reads its region's state, so that every tick is
  compared, not only what the wire samples at an entity's update interval. Items at rest
  are compared allowing for the phase of `getId()` (section 3.1). The harness gets an
  option to keep entities and a console after start.
- **By the game's bounds**: the Q throw's direction, an explosion's blocks (its rays are
  scaled by the level's random, and its drops shuffled), wandering, panic, path choice,
  spawn positions, loot counts, variants: checked against the rules and by
  distributions over many runs.
- **The wire**: the packets for an entity (type, data, the view) as the official server
  sends them, as the comparisons do now, with bots that follow relative moves, motion
  and entity data.

## 6. Questions for the owner

1. **How durable an entity's every tick must be.** Proposed: every tick's entity changes
   in the commit, so a crash loses no tick of any entity. With counters derived and
   randomness without state, an item or a standing animal costs nothing; a moving
   entity about 40 bytes a tick, **Inferred**; a mob with AI more, measured in N7. The
   cheaper rule: what matters (spawns, deaths, crossings, pick-ups, damage) at once and
   positions every second, so a crash moves mobs and items back up to a second.
   Recommended: the first, with the second kept for if the measurements show the log
   cannot keep up.
2. **Survival mode: where in the order?** The game is creative only. In creative,
   monsters still matter (they attack villagers and golems, burn in daylight, are hit by
   the player), but they never attack the player, nothing hurts the player, and there is
   no hunger, death or experience. Survival is not in the order agreed on 2026-10-15:
   before N9, after the phase, or later?
3. **Villagers.** The structures' entities are placed with their chunks in N7, so that a
   village is not empty, with simple goals; the villagers' brain, trading, golems and
   raids are a phase of their own (N10). Agreed?
4. **Light before monsters.** The agreed order puts light after entities. Animals made
   with the chunk and monsters' spawning need light, and monsters burn in daylight.
   Proposed: N1 to N7, then light in the simulation (N8), then N9 and N10. Agreed?
5. **What parity means for entities.** The official server never repeats a mob's
   wandering itself (its randoms are seeded from the clock). Parity is then the game's
   rules and their distributions, and exact where the game draws nothing (falling sand,
   a summoned item's flight, the animals generation makes). And three uses of HotSpot's
   `Math.sin`/`Math.cos` (the Q throw, elytra flight, TNT's priming) ported as a named
   exception, as the icebergs' are. Agreed?
6. **Difficulty.** The game's default is easy; peaceful removes monsters, and difficulty
   changes spawning and damage. A setting (`--difficulty`, default easy as the game's)?

**The owner's answers (2026-10-17).** 1: every tick. 2: survival later, with the rest of
parity by the matrix's tiers, after the Nether and the End. 4: light before monsters,
as proposed. 5: the game's rules and distributions, exact where the game draws nothing,
the three HotSpot uses a named exception. Questions 3 and 6 were not put; the
recommendations are taken and can be overruled: villagers' brain and trading a phase of
their own (N10), and a `--difficulty` setting, easy by default as the game's.

## 7. Risks

- **The log's growth** with many moving entities, and the copies' updates, which carry
  the same changes to every coupled neighbour (question 1); entities going back and forth
  across a pinned line cost an arrival each crossing.
- **Floating point**: `f32` and `f64` as Java, no fused operations; the three HotSpot
  uses (question 5).
- **The edge's map of keys to wire ids** under resumes and several edges, and every id
  inside packets.
- **AI across a line** reads copies a tick old: mobs that target or flock across a line
  may hesitate there.
- **An entity in flight** between regions is in neither for that time: missed by pressure
  plates, hoppers, mob counts and pick-ups in it.
- **The world-wide mob cap** is a value of the whole world, read from the store a few
  ticks late.
- **The size of the port**: 161 entity types and about 1,200 classes; the order of N4 to
  N10 puts first what a player meets first.
- **The tick's length**: path finding for many mobs in one region, measured in N7.

## 8. The first review (2026-10-17), and what changed

Against the code and the records, sections 3.1 to 3.5. Worked in:

- D1, the store writing an arrival into a stored chunk that a region still holds,
  undone by that region's restore: written only where no region holds the chunk, behind
  pending saves, once durable; kept filed otherwise, never dropped (3.4).
- D2, a key given again after a crash, with the edge showing the old type: a fresh
  block at every hello (3.1).
- G1: entities are part of the chunk, not of the region's opaque state; entity changes
  keyed by chunk and key, idempotent, a move between chunks as removal and whole (3.2,
  3.3).
- G2: an entity effect is addressed to the chunk the sender sees it in and to its key;
  the passing on with arrivals is gone (3.5).
- G3: pick-up only in the player's own region's land (3.5).
- G4: vehicles cross with their riders (3.4).
- G5: the edge's rule by `hops`, raised at every change of holder; the sender reports
  "handed on", never a removal; an entity handed out of view is removed; a player's own
  id stays mapped; `ID_QUIET` is gone (3.1, 3.4).
- G6: an entity moving into its own region's chunk that is not loaded is an arrival too
  (3.4).
- G7: arrivals not deduplicated, ready once their chunk is loaded, split when large, the
  taking commit carrying the entity whole (3.4).
- G8: keys at each hello and in a split's group, the home region's block above every
  stay (3.1).
- Q1: while coupled, an arrival is not delayed (3.4). Q2 and Q4: crossing back and forth,
  and the copies' updates, measured in N2 (4, 7). Q3: the records amended are named in
  N0 (below).

ADR-0028 is to name the records it amends: ADR-0022 (a chunk carries entities), ADR-0025
sections 1 and 5 (arrivals are not dropped), ADR-0024 section 6a (entities in the tick),
and ADR-0006's open question on versions (`hops`).

## 9. The second review (2026-10-17), and what changed

Against the game's classes, sections 3.6 to 7. Worked in:

- D1: the account of the game's randomness was wrong (each entity's random and the
  level's are seeded from the clock; the order of entities is deterministic): section 5
  is rewritten, the Q throw and the explosion's blocks are judged by bounds, and the
  three HotSpot uses are named (3.6, question 5).
- D2: the game's `getId()` is game logic: the sim's `id()` (3.1).
- D3: what changes in a tick is more than position: counters derived, randomness without
  state, mobs measured in N7 (3.3, question 1).
- D4: the world-wide mob cap, players within 128 blocks, spawning's reach beyond the
  copies: counts published by the store (3.7).
- D5: the animals made with a chunk follow the game's generation random and are judged
  exactly; they need light (3.7, question 4).
- D6: entities tick one ring inside the random-tick chunks (3.2).
- D7: entities' place in the tick and their order (3.2).
- G1: vehicles and what a creative player places are a step, N5.
- G2: riding is designed with crossing (3.4, N3).
- G3: the whole inventory in N2; the Q throw's pick-up delay and thrower, and dropping
  from the creative tab (4); a creative pick-up never fails (3.5).
- G4: the structures' entities placed with their chunks in N7, the brain later
  (question 3).
- G5: every id inside packets translated (3.1).
- G6: entities saved apart from the blocks (3.2).
- G7: `f32` where the game uses `float` (3.6).
- G8: the oracle compares every tick through the console (5).
- Its points on the questions: question 2 asks rather than recommends; question 5 is
  framed by the clock-seeded randoms; difficulty is question 6; the risks of AI across a
  line, of an entity in flight and of the world-wide cap are added.
