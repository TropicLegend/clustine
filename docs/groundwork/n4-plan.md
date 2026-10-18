# Step N4: physics, items and falling blocks, planned

Draft of 2026-10-18, revised the same day after an independent review (section 7). Step N4 of the
[plan of phase N](n-plan.md) and of [ADR-0028](../adr/0028-entities.md), brought before
N3 (crossing): until something moves there is nothing to carry across a line. What the
game does is in the [porting brief](n4-brief.md), read from the 26.3 jar; this plan says
how it is built and in what parts. A section of the brief is "brief §n".

## 1. What a player meets after N4

- A thrown item falls, bounces, slides on ice, is pushed by flowing water and floats in
  it, burns in lava and fire (but netherite), merges with others of its kind, and
  despawns after five minutes; a creative player walking over it picks it up after its
  two seconds, and others see the item fly to them.
- Sand, gravel, concrete powder and the other falling blocks fall as entities, at the
  game's pace, and land; one that cannot land drops as an item; concrete powder turns
  to concrete in water.
- Blocks broken by a rule drop what the game drops: a torch washed away, decayed leaves
  (saplings, sticks, apples by the game's chances), a block a piston or an update
  breaks, a sign or a flower whose support goes. A creative player's own break drops
  nothing, as the game's.
- Until N3, an entity that would move into a chunk its region does not have loaded
  and ticking stops at that chunk's edge: at a pinned line an item does not cross. That
  is written into the roadmap's "what to try" as the seam N3 mends.

## 2. Decisions taken (the brief's section 0)

- **A resting item's motion** (brief finding 5): its y motion cycles every tick and its
  position changes only every fourth. A tick commits an entity's motion only together
  with a change of its position, on-ground, data or fire, and every entity whole at a
  checkpoint. After a crash a resting item's motion may be that of a few ticks before,
  which nothing a client sees depends on.
- **Same-tick ticking** (finding 6): as the game, an entity added before the entity phase
  of a tick (a throw, a drop, a falling block starting) ticks in that tick; one added
  during the entity phase waits for the next. ADR-0028 section 2 is amended.
- **Pick-up** runs after the entity phase, as the game's players' tick does after the
  levels' (finding 7), in ascending player order.
- **Streams for drops** (brief §8.5): the loot's draws and the item's spawn offsets from
  one stream of the block (world seed, the block's position, the tick, a purpose tag),
  the new item's own draws (UUID, bob, yaw, motion) from its key's stream; a named
  difference (distributions the game's, values not).
- A **partial pick-up** happens only in survival, which is not built; in creative what
  does not fit is destroyed (`Inventory.add`).

## 3. Parts, in the order they are built

- **N4a, the data** (`tools/datagen`, `clustine-data`): through `Extract.java` the
  collision shapes in the game's voxel form (coordinate lists and filled cells, and
  which is `Shapes.block()`), each block's friction, speed factor, jump factor, bounce
  and fall-distance reduction, the entity-inside shapes of the five classes, and the
  entity type table (sizes, eye height, tracking range, update interval, delta tracking,
  fire immunity); from the generated data the items' components (maximum stack,
  damage resistance, maximum damage) and the block loot tables with their two
  predicate files, compiled into a compact table (brief §8.2). A subagent writes the
  extraction and the tables' readers; Java runs only by my hand (`cargo datagen`).
- **Shared contracts** (by me, after N4a): `EntityData` grows `Item { health, … }` and
  `FallingBlock { state, time, drop_item, cancel_drop }`, `Entity` grows `fall_distance`
  and `remaining_fire_ticks`; their encodings in `clustine-format` (the format raised);
  the region's reports to the edge for entities other than players: a move with the
  flags the edge's tracking needs (brief §9: collisions, needs-sync, sync position and
  velocity, precise position), a removal with `hops`, a pick-up (`TakeItemEntity`), a
  transient block of a falling block's landing.
- **N4b, movement and the item** (`clustine-sim`, `clustine-blocks`): the voxel collider
  (`Shapes.collide`, `BlockCollisions`, the supporting block), `Entity.move`
  (brief §1), the inside-block walk and the effects of blocks touched (fire, lava,
  cactus, the bubble columns as far as items meet them), `EntityFluidInteraction` and
  the flow vector (`fluid::flow`, brief §4), `ItemEntity.tick` whole (brief §3:
  gravity, drag, bounce, `restituteMovementAfterCollisions`, the fourth-tick gate by
  `getId()`, merging, despawning, damage and burning), pick-up and `Inventory.add`
  (brief §5); moves between chunks as `Gone` and `Whole`; the stop at land not loaded
  (section 1).
- **N4c, falling blocks and drops** (`clustine-sim`): `FallingBlock.tick` making a
  `FallingBlockEntity` in place of today's instant landing (`region/ticks.rs` `fall`),
  its tick (brief §7), landing, the drop of one item when it cannot; the loot evaluator
  for a block broken by a rule (brief §8.3: no tool, no entity, no explosion) and
  `popResource` (brief §8.4) in `destroy_block` where its flag asks for drops, in the
  fluids' washing away and in leaf decay.
- **N4d, the edge** (`services/edge`, `clustine-protocol`): `ServerEntity`'s bookkeeping
  per entity and viewer (brief §9: the update interval, the delta encoding,
  `MoveEntity.Pos`/`PosRot` with `VecDelta`, teleports, `SetEntityMotion` when delta
  tracking says), `TakeItemEntity`, `AddTransientBlock`; compared with the official
  server (a dropped item falling onto a slab and into water; a block of sand falling),
  by layout and when the packets come, as the protocol rule asks.
- **N4e, the measurement**: the log's bytes with items falling and lying, a stream of
  items in water, sand falling; the tick's length with a thousand items.

N4b and N4c share the sim and go in order or with crates split by module; N4d goes beside
them once the shared contracts are in.

## 4. What the oracle judges exactly

- A `/summon` of an item with a given motion and pick-up delay, and of a falling block,
  compared tick by tick through the console (`/tick freeze`, `/tick step`,
  `/data get entity … Pos Motion OnGround`), as the plan of phase N section 5 says: the
  movement code draws nothing, so position and motion are equal to the bit.
- The packets the official server sends for them (brief §9), by layout and timing.
- By bounds: drops from loot tables (counts within the table's ranges, chances over
  many runs).

## 5. Tests

From this plan and the brief by someone who did not build it: an item falls onto every
kind of shape (slabs, stairs, fences, a cauldron, a scaffold) and stops where the game's
does (from the oracle's numbers, worked out by hand for a few); slides on ice and
bounces on slime as the brief's constants say; is pushed by flowing water along the
game's flow; merges and despawns at the game's ages; is picked up after its delay;
survives every interruption as N2's did; sand falls and lands, and drops when it cannot;
leaves decay into their loot.

## 6. Questions for the reviewer

- Is the brief's rule for what a resting item commits (section 2) sound with replay,
  copies and the edge's tracking?
- Does the stop at land not loaded (section 1) leave anything able to fall out of the
  world or into a chunk it should not, at a lend, a merge or a split?
- Is anything a player meets within minutes missing (pick-up sounds are the client's;
  the item's glint, its bobbing are the client's)?

## 7. The review (2026-10-18), and what changes

Ready after fixes; these replace the sections above where they differ. The review
sampled the brief against the jar at a dozen points and found every one to hold.

- **Counters and motion reach the store without a commit** (its D1, a defect N2 already
  has): the worker saves only chunks with committed changes, so a resting item's age,
  pick-up delay and motion never reached the store at a stop, a merge, a split or a
  return. The sim reports the chunks whose entities changed in a tick without a commit
  (`TickOutput::entities_ageing`, as `ticks_pending` is), the worker keeps them and saves
  them with `Parts::ENTITIES` at every checkpoint and at a stop, and a warm chunk stays
  what the store has. In the shared contracts, with a worker test: an item rests, the
  region stops, the store has its age and motion.
- **The stop at land not loaded is a guard after every move** (D2): after a move,
  `noPhysics` and `moveTowardsClosestSpace` included, an entity whose chunk would be one
  the region does not tick keeps its old coordinate on that axis and that component of
  its motion is zeroed. Tested with a throw at x = 15.95 towards a line, an item pushed
  out of a block placed on it, and water carrying an item to a pinned line.
- **`tickCount` is derived from the region's tick** (D3), as an `i32` wrapping, so that
  a region and its restore gate an item's fourth-tick moves, its merging and its sounds
  on the same ticks (ADR-0014); the game resets it at a load, which only shifts the
  phase: a named difference. So is the stuck-speed multiplier lost for one tick after a
  restore (a cobweb). A restore may mix fields of different ticks (older committed motion
  with newer counters from a file), harmless and named.
- **Pick-up after the block entities** (D4): the game ticks each level whole, block
  entities included, before the connections; it matters once hoppers come.
- **Copies** (G1): collision and the fluid test count copied chunks as loaded, so that an
  item a block from a line does not sink through water; merging and pick-up leave out
  entities in copies (ADR-0028 section 7).
- **The region's reports to the edge** (G2), in the shared contracts: a move with the
  sync flags; a data change (the stack after a merge, index 8; the on-fire flag, index 0);
  a falling block's spawn with its state and start position (its index 8, a block
  position); `EntityRemoved` with `hops`; `EntityField` variants for fall distance, fire
  ticks and health. A resting item is reported only with its committed changes or its
  sync flags, so its `SetEntityMotion` may differ from the game's and is left out of the
  timing comparison: named. The order of packets in a tick is added to the brief:
  `sendChanges` before the tick's entity phase, `AddEntity` at once (a falling block
  appears before the air its block leaves), `TakeItemEntity` in the connections' tick,
  `BlockUpdate` and `AddTransientBlock` at once.
- **What commits** (G3): counters (age, pick-up delay, the fire countdown) do not make a
  commit; health and the stack do; a merge's smaller age and larger delay are in its `Set`.
- **Drops** (G4) need their chunk loaded, not ticking; into a full chunk or with no key
  left a drop is lost, named. The block's stream counts its draws per position and tick,
  as `entity_draws` does, so that two drops at one block in one tick differ.
- **Pistons push entities** (G5), as the plan of phase N has it in N4: a part of its own,
  N4f, after N4c (`MoverType.PISTON`, `limitPistonMovement`).
- **Order** (section 3): N4c after N4b (it needs `Entity.move` and both edit the tick),
  with the loot evaluator in a module of its own (`clustine-blocks::loot`) that can be built
  beside N4b; N4d from the contracts, its comparisons once N4b and N4c are in.
- **What a player meets** (Q1): until N3 an entity stops at every line between regions,
  also those of regions that follow players, where nothing is then sent across that could
  make them merge. The owner tries at the end of a phase and N3 comes right after N4, so
  the seam is gone before then; written into the roadmap's "what to try" with the rest
  that waits: hoppers (N6), pick-up across a line (N3), a partial pick-up (survival).
