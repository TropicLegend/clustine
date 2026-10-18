# N4 porting brief: physics, items and falling blocks (26.3, protocol 777)

Read from `server-26.3.jar` with `javap -p -c -constants` (names are Mojang's) and from
`target/datagen/26.3/generated/`. Every rule below is the bytecode's unless marked
**Inferred** (reasoning, to test) or **Client** (client code, not in the server jar).
Method names are the game's; "line" ranges, where given, are of the `javap` output of
that class, so a porter can go straight to them:

```
J=target/datagen/26.3/versions/26.3/server-26.3.jar
$HOME/.local/jdk/bin/javap -p -c -constants -cp $J net.minecraft.world.entity.Entity
```

Conventions throughout: `double` is `f64`, `float` is `f32`; a `float` constant used in
`double` arithmetic is widened (`f2d`) *after* any float arithmetic it takes part in;
no fused multiply-add (Rust does not contract `a*b+c` by itself; never call
`mul_add`); `Mth.floor(double)` is `(int)Math.floor(x)`, which Rust's `x.floor() as i32`
matches (both saturate, NaN gives 0); Java `int` arithmetic wraps and `%` takes the
sign of the dividend (`i32::wrapping_add`, Rust `%`).

---------------------------------------------------------------------------------------

## 0. What the step should know before planning (findings)

These change what the roadmap and ADR-0028 assume; each is detailed in its section.

1. **26.3 moved and rewrote parts of the physics.** Fluid pushing is no longer in
   `Entity.updateFluidHeightAndDoFluidPushing`; it is `EntityFluidInteraction`
   (`update`, `applyCurrentTo`, its `Tracker` and `CurrentAccumulator`). Bouncing is
   `Entity.restituteMovementAfterCollisions` with a per-block `bounceRestitution`
   (slime, beds, the shelf mushroom), not `Block.updateEntityMovementAfterFallOn`.
   `FlowingFluid.getFlow` reads a block tag `minecraft:blocks_fluid_flow`. The move
   packets carry a `VecDelta`/`PositionPath` (section 9).
2. **Items are pushed by currents twice a tick**: once in `Entity.baseTick` and once at
   the end of `ItemEntity.tick` (`needsSync |= updateFluidInteraction()`).
3. **A falling block that cannot land drops one of its block's item**
   (`spawnAtLocation(level, block)`, i.e. `new ItemStack(block.asItem())`), *not* its
   loot table. Loot tables are for blocks broken (section 8).
4. **A falling block does not call `super.tick()`**: no `baseTick`, so no fluid
   tracking, no pushing by water, no fire ticks, no fall below the world's floor check
   other than its own `time > 100` rule.
5. **An item lying still changes its motion every tick.** On ground and slow it moves
   only when `(tickCount + getId()) % 4 == 0`, but gravity is added every tick, so its
   `y` motion runs `0, -0.04, -0.08, -0.12, (-0.16 → collided to 0)`; and its tiny
   horizontal motion is multiplied by `0.98f·friction` on each fourth tick for the
   rest of its life (subnormal for most of it). Its position does not change
   (`Entity.move` refuses a move that collision cut down to under `1e-7` squared while
   the request was larger). ADR-0028's "an item lying on the ground commits nothing"
   therefore needs a rule for motion (section 3.5 proposes one).
6. **In the game an entity spawned before the entity phase ticks in that same tick.**
   `EntityTickList.add` puts it in the active map at once; only entities added *while*
   the list is being iterated (the entity phase itself) wait for the next tick
   (`ensureActiveIsNotIterated` copies the map). So sand that starts falling in a
   scheduled tick moves 0.04 down in the same tick, and an item a cactus or a piston
   drops ticks in the tick it was dropped. ADR-0028 section 2 says "one spawned in the
   tick ticks first in the next": a difference from the game to name or remove.
7. **Pick-up runs after the level's tick.** `MinecraftServer.tickChildren` ticks every
   level, then `tickConnection`, where `ServerPlayer.doTick` → `Player.aiStep` picks
   up. So an item that lands in a tick can be picked up in that tick, after the
   entities.
8. **Survival `Inventory.add` returns false after a partial fit** (the loop's last pass
   made no progress), so `ItemEntity.playerTouch` sends no `TakeItemEntity` though the
   stack on the ground was shrunk in place; and because `setItem` is not called the
   item's `DATA_ITEM` is not marked dirty, so **Inferred**: clients are not told of the
   new count. Creative destroys what does not fit (ADR-0028 section 7 holds).
9. **The pick-up sound is the client's** (**Client**): the server sends only
   `TakeItemEntity`; the client plays `entity.item.pickup` (volume 0.2, pitch
   `(r.nextFloat() - r.nextFloat()) * 1.4 + 2.0` from its own random) and animates.
   No server random is drawn for it.
10. **Collision needs the game's voxel form of shapes, not boxes.** The repo keeps a
    shape as the boxes of `forAllBoxes`; `VoxelShape.collide` walks the shape's grid
    (coordinate lists and filled cells), which differs from a box list where the
    entity already overlaps a shape and where the grid has lines inside a merged box.
    Extract the grid (section 11).
11. **Loot tables changed format in 26.3**: entries have `"modifier"` (not
    `"functions"`), a condition is a single `"condition"` that may be a string naming
    a predicate file (`minecraft:tool/can_silk_touch`, `minecraft:tool/can_shear` in
    `data/minecraft/predicate/tool/`), and `block_state_property` is now
    `minecraft:match_block`. The full census is in section 8.
12. `ItemEntity`'s `y *= -0.5` bounce on ground almost never fires now: the restitution
    of section 1.7 sets `y` to `+0.0` on a downward collision first, so `y < 0` is
    false. Port it anyway.
13. The world's random sequences: block loot draws from `server.getRandomSequence(
    "minecraft:blocks/<name>")`, a saved per-table stream; `popResource` from the
    level's random. Neither is reproducible in Clustine; parity by distribution
    (ADR-0028 section 4). Section 8.5 proposes the streams.

---------------------------------------------------------------------------------------

## 1. `Entity.move` and collision

### 1.1 Order of a tick around `move`

`ServerLevel.tickNonPassenger(entity)`: `entity.commonTick()` then `entity.tick()`.
`commonTick`: `invulnerableTime--` if > 0; `setOldPosAndRot()` (xo/yo/zo = position,
old rotations); **`tickCount++`**. So `tickCount` is already 1 in an entity's first
`tick()`. `tickCount` is not saved: a loaded entity starts at 0 (so does `firstTick =
true`). Clustine's take-in of an entity (load, arrival, warm chunk) is the game's load:
`tick_count` can be derived as "region ticks since this region took it in" and needs no
storing.

### 1.2 `Entity.move(MoverType.SELF, Vec3 vec)` (Entity.txt lines 1491–1840)

1. (IDE-only check; skip.)
2. **`noPhysics`**: `setPos(x+vec.x, y+vec.y, z+vec.z)`; `recordMovement`; set
   `horizontalCollision = verticalCollision = verticalCollisionBelow =
   minorHorizontalCollision = false`; return. (No `addMovementThisTick`, no collision.)
3. `MoverType.PISTON` only: `limitPistonMovement` (N4's pistons; not items' own moves).
4. **Stuck speed**: if `stuckSpeedMultiplier.lengthSqr() > 1e-7`: (if not PISTON)
   `vec = vec.multiply(stuckSpeedMultiplier)` (component-wise); then
   `stuckSpeedMultiplier = ZERO` and `deltaMovement = ZERO`.
5. `vec = maybeBackOffFromEdge(vec, type)`: identity for items and falling blocks.
6. `out = collide(vec)` (1.3). `d5 = out.lengthSqr()`.
7. **Accept the move** iff `d5 > 1e-7 || vec.lengthSqr() - d5 < 1e-7`:
   - if `fallDistance != 0 && d5 >= 1.0`: `d7 = min(out.length(), 8.0)`; `to =
     position + out.normalize().scale(d7)`; `level.clip(ClipContext(position, to,
     Block.FALLDAMAGE_RESETTING, Fluid.WATER, this))`; if the hit is not `MISS`:
     `resetFallDistance()` (`fallDistance = 0`). `FALLDAMAGE_RESETTING` gives a full
     block for states in `#minecraft:fall_damage_resetting`, empty otherwise.
     (Only items moving a block a tick or more meet this; the repo has a block
     traversal and clip in `crates/clustine-sim/src/placement/bucket.rs`.)
   - `from = position; to = from + out`; `addMovementThisTick(Movement(from, to,
     vec))` (1.9); `setPos(to)` (which also remakes the box, 1.4); `recordMovement`.
   Otherwise the position does not change and no movement is recorded.
8. `xColl = !Mth.equal(vec.x, out.x)`, `zColl = !Mth.equal(vec.z, out.z)`
   (`Mth.equal(a,b)` = `Math.abs(b - a) < 9.999999747378752E-6` (`1.0E-5F`
   widened, `0x3ee4f8b580000000`)). `horizontalCollision = xColl || zColl`.
9. `movedY = Math.abs(vec.y) > 0`. On the server `isLocalInstanceAuthoritative()` is
   true for anything without a controlling passenger, so always:
   `verticalCollision = vec.y != out.y` (exact `!=`);
   `verticalCollisionBelow = verticalCollision && vec.y < 0`;
   `setOnGroundWithMovement(verticalCollisionBelow, horizontalCollision, out)` →
   `onGround = verticalCollisionBelow; horizontalCollision = …;
   checkSupportingBlock(onGround, out)` (1.5).
10. `minorHorizontalCollision = horizontalCollision && isHorizontalCollisionMinor(out)`
    (false for non-living entities: only `LivingEntity` overrides; Entity's returns
    false).
11. `onPosLegacy = getOnPosLegacy()` (`getOnPos(0.2f)`, 1.5); `stateBelow =
    getBlockState(onPosLegacy)`; `checkFallDamage(out.y, onGround, stateBelow,
    onPosLegacy)` (1.6).
12. If removed now, return.
13. `canSimulateMovement()` is true on the server for `SERVER_AND_CLIENT`
    (items and falling blocks both). If `(movedY && verticalCollision) ||
    horizontalCollision`: `restituteMovementAfterCollisions(stateBelow, xColl, zColl,
    out)` (1.7).
14. Movement emission (step sounds, vibrations): items and falling blocks return
    `MovementEmission.NONE`, so nothing.
15. `f = getBlockSpeedFactor()` (1.8); `deltaMovement =
    deltaMovement.multiply((double)f, 1.0, (double)f)`.

`recordMovement` feeds the client-prediction interpolation handler; for items and
falling blocks the handler is `InterpolationHandler.NO_OP` (neither overrides
`createInterpolationHandler`): nothing to port.

### 1.3 `collide(vec)` (lines 2860–3045) and step-up

```
bb = getBoundingBox()
entityColliders = level.getEntityCollisions(this, bb.expandTowards(vec).expandTowards(0, maxUpStep, 0))
out = vec.lengthSqr() == 0 ? vec : collideBoundingBox(this, vec, bb, level, entityColliders)
xc = vec.x != out.x; yc = vec.y != out.y; zc = vec.z != out.z      // exact
downHit = yc && vec.y < 0
if maxUpStep() > 0 && (downHit || onGround()) && (xc || zc): … step-up …
return out
```

`maxUpStep()` is 0 for items and falling blocks (`Entity` default; only living
entities, minecarts and boats raise it), so the step-up branch never runs for them. For
later steps it is: `base = downHit ? bb.move(0, out.y, 0) : bb`; `search =
base.expandTowards(vec.x, maxUpStep, vec.z)`, and if not `downHit`
`.expandTowards(0, -9.999999747378752E-6, 0)`; colliders of `search`; heights =
`collectCandidateStepUpHeights(base, colliders, maxUpStep, (float)out.y)` (each Y
coordinate of each collider minus `base.minY`, as float, kept if `>= 0`, `!= out.y`
float and `<= maxUpStep` (breaking out of that collider at the first above it), a
`FloatArraySet` then `FloatArrays.unstableSort`); for each height `h` ascending: `r =
collideWithShapes(Vec3(vec.x, h, vec.z), base, colliders)`; if
`r.horizontalDistanceSqr() > out.horizontalDistanceSqr()` return `r.subtract(0,
bb.minY - base.minY, 0)`.

`getEntityCollisions(entity, box)`: if `box.getSize() < 1e-7` empty; otherwise the
collision boxes of entities that `canBeCollidedWith(entity)` (boats, minecarts,
shulkers, happy ghasts…). **None exist in N4**; keep the parameter and pass nothing.

`collideBoundingBox(entity, vec, bb, level, entityColliders)`: `colliders =
collectCollidersIgnoringWorldBorder(entity, level, entityColliders,
bb.expandTowards(vec))` then `collideWithShapes(vec, bb, colliders)`.

`collectCollidersIgnoringWorldBorder`: the entity colliders first, then the world
border's shape if `worldBorder.isInsideCloseToBorder(entity, box)`, then
`level.getBlockCollisions(entity, box)` (1.4), in that order.

**`collideWithShapes(vec, bb, shapes)`** (lines 3232–3277): if `shapes` is empty
return `vec`. `acc = ZERO`. For each axis of `Direction.axisStepOrder(vec)`:
`Math.abs(vec.x) < Math.abs(vec.z)` → **Y, Z, X**, otherwise **Y, X, Z**. Skip an
axis whose `vec` component is exactly 0. `d = Shapes.collide(axis, bb.move(acc),
shapes, vecComponent)`; `acc = acc.with(axis, d)`. Return `acc`.

**`Shapes.collide(axis, box, shapes, d)`**: for each shape in order: if
`Math.abs(d) < 1e-7` return `0.0`; `d = shape.collide(axis, box, d)`. Return `d` (so
a tiny `d` after the last shape is returned as it is).

**`VoxelShape.collide(axis, box, d)`** = `collideX(AxisCycle.between(axis, X), box,
d)` (VoxelShape.txt lines 601–830):

```
if isEmpty(): return d
if |d| < 1e-7: return 0
inv = cycle.inverse(); a = inv.cycle(X); b = inv.cycle(Y); c = inv.cycle(Z)
maxA = box.max(a); minA = box.min(a)
lo  = findIndex(a, minA + 1e-7);  hi = findIndex(a, maxA - 1e-7)
b0 = max(0, findIndex(b, box.min(b) + 1e-7)); b1 = min(size(b), findIndex(b, box.max(b) - 1e-7) + 1)
c0 = max(0, findIndex(c, box.min(c) + 1e-7)); c1 = min(size(c), findIndex(c, box.max(c) - 1e-7) + 1)
n = size(a)
if d > 0:
  for i in hi+1 .. n-1:  for j in b0..b1-1:  for k in c0..c1-1:
     if isFullWide(inv, i, j, k):            // cell (i,j,k) in the (a,b,c) frame
        e = coord(a, i) - maxA
        if e >= -1e-7: d = min(d, e)
        return d
elif d < 0:
  for i in lo-1 down to 0:  (same j, k loops)
     if isFullWide(inv, i, j, k):
        e = coord(a, i + 1) - minA
        if e <= 1e-7: d = max(d, e)
        return d
return d
```

`findIndex(axis, v)` = `Mth.binarySearch(0, size+1, i -> v < coord(axis, i)) - 1`
(the last `i` with `coord(i) <= v`; `binarySearch` returns `to` when none). `coord`
of a moved shape is `base coord + offset` in double (`OffsetDoubleList`).
`isFullWide(cycle, x, y, z)` reads the discrete shape's cell at the position the cycle
maps `(x, y, z)` to. Note the early `return d` at the first full cell found along the
axis, whether or not it lowered `d`.

### 1.4 Boxes, block collisions and shapes

- **Bounding box**: `EntityDimensions.makeBoundingBox(x, y, z)`: `f = width / 2.0F`
  (float), `h = height` (float); `AABB(x - f, y, z - f, x + f, y + h, z + f)` with
  `f`, `h` widened. Items `0.25f × 0.25f` (`f = 0.125`), falling blocks `0.98f ×
  0.98f` (`f = 0.49f` = `0.49000000953674316`, `h = 0.9800000190734863`).
  `setPos` remakes the box from the new position every time. The `AABB` constructor
  sorts each pair (`Math.min`/`Math.max`).
- `AABB.expandTowards(x, y, z)`: negative components lower the min, positive raise the
  max; `inflate(x,y,z)`: `min - x … max + x` through the sorting constructor;
  `deflate(d)` = `inflate(-d)`; `intersects` is strict on all six.
- **`BlockCollisions(level, entity, box, onlySuffocating=false, (pos, shape) ->
  shape)`** (BlockCollisions.txt): context `CollisionContext.of(entity)`
  (an `EntityCollisionContext(entity, false, false)` for anything but a minecart);
  the cells are a `Cursor3D` over `floor(minX - 1e-7) - 1 … floor(maxX + 1e-7) + 1`,
  likewise y and z, **x fastest, then y, then z**. A cell's "type" counts how many of
  its coordinates are at the cursor's first or last value: type 3 (a corner) is
  skipped; type 1 is kept only if `state.hasLargeCollisionShape()`; type 2 only if the
  state is `minecraft:moving_piston`. Chunks come from `getChunkForCollisions(cx,cz)`
  (`getChunk(cx, cz, FULL, false)`): a chunk not loaded gives no collisions at all
  (Clustine: neither loaded nor copied). For each cell: `shape =
  context.getCollisionShape(state, level, pos)`; if `shape == Shapes.block()` (the
  full cube instance) it is yielded, moved to `pos`, iff
  `box.intersects(x, y, z, x+1, y+1, z+1)`; otherwise `moved = shape.move(pos)`,
  yielded iff not empty and `Shapes.joinIsNotEmpty(moved, Shapes.create(box), AND)`
  (some full cell of the shape overlaps the box with positive volume on all three
  axes; `Shapes.create` of a box thinner than `1e-7` on an axis is empty, so then
  nothing is yielded).
- `hasLargeCollisionShape` (`BlockStateBase` cache) is whether the collision shape (as
  the repo extracts it, with the empty context) has a `min < 0` or `max > 1` on any
  axis: fences, walls, fence gates closed, moving piston. Derivable from the
  extracted shape.
- **Shapes that depend on the entity** (the repo's table holds the empty context's
  answer; `EntityCollisionContext` has `descending = entity.isDescending()` (shift key;
  false for items), `entityBottom = entity.getY()`, `placement = false`, held item
  empty for non-living, and `isAbove(shape, pos, canAscend) = entityBottom > pos.y +
  shape.max(Y) - 9.999999747378752E-6`):
  - **Powder snow** (`PowderSnowBlock.getCollisionShape`): entity `fallDistance >
    2.5` → `FALLING_COLLISION_SHAPE` = `box(0, 0, 0, 1, 0.8999999761581421, 1)`; a
    `FallingBlockEntity` → the full block; otherwise (items: not in
    `#minecraft:powder_snow_walkable_mobs`) **empty**. The empty context gives empty.
  - **Scaffolding**: `isAbove(Shapes.block(), pos, true) && !descending` →
    `SHAPE_STABLE`; else if `distance != 0 && bottom && isAbove(SHAPE_BELOW_BLOCK,
    pos, true)` → `SHAPE_UNSTABLE_BOTTOM`; else empty. The empty context's `isAbove`
    returns `canAscend` (true), so the repo's table has the stable shape: items below
    the top or beside it must get empty instead.
  - **Liquids** collide only with `alwaysCollideWithFluid` contexts or a mob that can
    stand on the fluid: never items or falling blocks.
  - **Shulker boxes and moving pistons** answer by their block entity (the repo: the
    closed box, and nothing for a moving piston); **bamboo, pointed dripstone and the
    sulfur spike** are offset by position (`BlockStateBase.getOffset`, from
    `Mth.getSeed(x, 0, z)`; `clustine_noise::random::get_seed` exists): the table holds
    the shape at (0,0,0) and the offset must be added.
- **World border**: `isInsideCloseToBorder(entity, box)`: `d = max(Mth.absMax(
  box.getXsize(), box.getZsize()), 1.0)`; true iff `getDistanceToBorder(entity) < 2*d`
  and `isWithinBounds(x, z, d)`. The default border (centre 0,0, size 5.9999968E7)
  is at ±29,999,984; Clustine has no border setting. Port it as the default border's
  shape (everything outside the square) or name it: an item within two blocks of
  the border is the only case.

### 1.5 Supporting block and the "on" positions

`checkSupportingBlock(onGround, movement)` (lines 1406–1484):

```
if onGround:
   probe = AABB(bb.minX, bb.minY - 1.0E-6, bb.minZ, bb.maxX, bb.minY, bb.maxZ)
   found = level.findSupportingBlock(this, probe)
   if found.isPresent() || onGroundNoBlocks: mainSupportingBlockPos = found
   elif movement != null:
        mainSupportingBlockPos = found = findSupportingBlock(this, probe.move(-movement.x, 0, -movement.z))
   onGroundNoBlocks = found.isEmpty()
else:
   onGroundNoBlocks = false; mainSupportingBlockPos = empty
```

`findSupportingBlock(entity, box)` (CollisionGetter lines 344–394): over the
`BlockCollisions` cells of `box` in their order, the position nearest the entity's
position by `pos.distToCenterSqr(position)` (`(pos.x + 0.5 - x)² + (pos.y + 0.5 - y)²
+ (pos.z + 0.5 - z)²`); on an exact tie the one **greater** by `BlockPos.compareTo`
(by y, then z, then x) wins.

`getOnPos(float f)`: if `mainSupportingBlockPos` is present: if `f > 1.0E-5F`:
`s = state at that pos`; if `(f <= 0.5 && s in #minecraft:fences) || s in
#minecraft:walls || s.getBlock() instanceof FenceGateBlock` return the pos; else return
`pos.atY(floor(y - (double)f))`; if `f <= 1.0E-5F` return the pos. Without a
supporting block: `BlockPos(floor(x), floor(y - (double)f), floor(z))`.

- `getOnPosLegacy()` = `getOnPos(0.2f)` (`0.20000000298023224`): fall damage, `fallOn`,
  the state given to restitution, `stepOn`.
- `getBlockPosBelowThatAffectsMyMovement()` = `getOnPos(0.500001f)` for most entities;
  **`ItemEntity` overrides it with `getOnPos(0.999999f)`** (`0.9999989867210388`,
  `0x3feffffde0000000`): friction and speed factor. So an item lying on a bottom
  slab takes its friction from the block *under* the slab (`floor(y - 0.999999)`),
  the game's quirk.

`mainSupportingBlockPos` and `onGroundNoBlocks` are not saved by the game (a loaded
entity has none until its next move): keep them in memory, cleared at take-in.

### 1.6 `checkFallDamage(movedY, onGround, state, pos)` (lines 3990–4043)

```
if !isInWater() && movedY < 0: fallDistance -= (double)(float)movedY
if onGround:
   if fallDistance > 0:
       state.getBlock().fallOn(level, state, pos, this, fallDistance)
       level.gameEvent(HIT_GROUND, …)              // vibrations: skip
   resetFallDistance()
```

`fallOn` for items and falling blocks, by block (`Block.fallOn` default:
`causeFallDamage(fallDistance * (1 - fallDistanceReduction), 1.0F, fall)`; an item's
`causeFallDamage` does nothing but pass to passengers):
- **Farmland** (`FarmlandBlock.fallOn`) draws `level.random.nextFloat()` *before* it
  checks for a living entity: a draw with no effect for items and falling blocks.
- **Honey block**: `entity.playSound(HONEY_BLOCK_SLIDE, 1.0F, 1.0F)` and, on the server,
  `level.broadcastEntityEvent(entity, 54)` (slide particles), for any entity.
- **Falling block**: `FallingBlockEntity.causeFallDamage` hurts living entities only
  where `hurtEntities` (anvils, the stalactite tip): left out with anvils' damage.
- Slime, hay, pointed dripstone, turtle egg, powder snow, beds: nothing for items.

Elsewhere: `baseTick` halves `fallDistance` in lava; `updateFluidInteraction` resets it
in water; `makeStuckInBlock` and the honey slide reset it.

### 1.7 `restituteMovementAfterCollisions(state, xColl, zColl, out)` (lines 1857–2000)

```
d5 = isSuppressingBounce() ? 0 : getEntityBounciness()     // items, falling blocks: 0 (no shift key)
v = getDeltaMovement(); w = v
if xColl: w.x = -v.x * d5
if zColl: w.z = -v.z * d5
bounced = d5 > 0 && (xColl || zColl)
if verticalCollision:
   if verticalCollisionBelow:
       d5 = (-v.y <= getEffectiveGravity() || isSuppressingBounce() || state in #minecraft:suppresses_bounce)
              ? 0 : max(d5, getBlockBounciness(state.getBlock()))
   if d5 > 0:
       r = out.y / v.y
       a = r * getEffectiveGravity()
       b = Mth.lerp(r, 1.0, (double)getAirDrag())            // 1 + r * (airDrag - 1)
       bounced = true
   else: a = 0; b = 1
   w.y = (a - v.y) * b * d5
if bounced:
   state.getBlock().bounceOn(level, state, getBlockPosBelowThatAffectsMyMovement(), this, fallDistance)
   gameEvent(BOUNCE); syncPosition = true                    // syncPosition matters to the edge (section 9)
setDeltaMovement(w)
```

`getBlockBounciness(block)` = `block.getBounceRestitution()` (float), times `0.8f` for
anything not a `LivingEntity`, widened. The bounce restitution is `0` but for: **slime
block 1.0**, **every bed 0.75**, **shelf mushroom 0.75** (to confirm by extraction).
So for an item: slime 0.8, bed or shelf mushroom 0.6 (as float `0.75f*0.8f` widened).
`getEffectiveGravity()` = `getGravity()` (0.04 unless no-gravity). `bounceOn` does
something only on the shelf mushroom (a sound, not for items or primed TNT).

Note the zeros' signs: with `d5 = 0` a horizontal collision gives `w.x = -v.x * 0` and
a downward collision `w.y = (0 - v.y) * 1 * 0 = +0.0`. Port the expression, not "0".

### 1.8 Speed factor and friction

`getBlockSpeedFactor()`: `s = state at blockPosition()`; `f =
s.getBlock().getSpeedFactor()`; if `s` is `minecraft:water` or `minecraft:bubble_column`
return `f`; else if `(double)f != 1.0` return `f`; else return the speed factor of the
block at `getBlockPosBelowThatAffectsMyMovement()`.

`Block.getFriction()`, `getSpeedFactor()`, `getJumpFactor()`, `getBounceRestitution()`
and `getFallDistanceReduction()` are plain fields set by `BlockBehaviour.Properties`
(defaults **friction 0.6f, speed 1.0f, jump 1.0f, bounce 0, reduction 0**). The values
set in `Blocks` (to confirm by extraction, section 11):

| block | friction | speed | jump | bounce | fall reduction |
|---|---|---|---|---|---|
| ice, packed ice, frosted ice | 0.98f | | | | |
| blue ice | 0.989f | | | | |
| slime block | 0.8f | | | 1.0f | |
| soul sand | | 0.4f | | | |
| honey block | | 0.4f | 0.5f | | |
| every bed (`Blocks` lambda for beds) | | | | 0.75f | 0.5f |
| shelf mushroom | | | | 0.75f | 0.5f |

Ground friction of an item (`ItemEntity.tick`, section 3): float `0.98f * friction`,
e.g. `0.98f*0.6f = 0.5880000591278076` (`0x3f16872c` as float), ice `0.9604000449180603`
(`0x3f75dcc7`), blue ice `0.9692200422286987` (`0x3f781ece`), slime `0.784000039100647`
(`0x3f48b43a`).

### 1.9 Movements of the tick and blocks touched (`applyEffectsFromBlocks`)

`addMovementThisTick(m)`: a deque of at most 100; when full the first two are merged
into one `Movement(first.from, second.to)` (no axis vector) put back at the front, then
`m` is appended. `Movement(from, to, vec)` keeps `vec` as its
`axisDependentOriginalMovement`; `Movement(from, to)` has none.

`applyEffectsFromBlocks()` (lines 2144–2204): `final = movementThisTick` (copied, then
the deque cleared); if `final` is empty, `final = [Movement(oldPosition(),
position())]`; else if `final.last.to.distanceToSqr(position()) >
9.999999439624929E-11` append `Movement(final.last.to, position())`. Then the private
`applyEffectsFromBlocks(final)`.

`applyEffectsFromBlocksForLastMovements()` reruns it with the previous `final` list
(an item that did not move this tick). Not saved by the game: empty after a take-in. A
resting item's last move tick has its move refused (1.2 step 7), so its `final` is
`[Movement(p, p)]` at its own position: Clustine can derive it.

`applyEffectsFromBlocks(list)` (lines 2294–2377): return if `!isAffectedByBlocks()`
(`isRemoved() || noPhysics`). If `onGround()`: `pos = getOnPosLegacy()`; `state.
getBlock().stepOn(level, pos, state, this)`. Then `wasOnFire = isOnFire()`,
`wasFreezing = isFreezing()`, `fireBefore = getRemainingFireTicks()`;
`checkInsideBlocks(list, insideEffectCollector)`; `insideEffectCollector.
applyAndClear(this)`; if `isInRain()` `clearFire()` (Clustine has no weather: name it);
the extinguish sound if it was on fire and no longer is (or freezing stopped);
`ignited = getRemainingFireTicks() > fireBefore`; on the server, if `!isOnFire() &&
!ignited`: `setRemainingFireTicks(-getFireImmuneTicks())` (`0` for these entities).

`stepOn` that act on items (any entity not stepping carefully, which items never do):
- **Slime block**: `|v.y| < 0.1` → `k = 0.4 + |v.y| * 0.2`; `v = v.multiply(k, 1.0, k)`.
- **Redstone ore**: `interact` → lights it (particles, `lit=true` if not lit). An item
  lying on redstone ore keeps it lit. (Clustine: if redstone ore's light-up exists for
  players, call it.)
- Magma (living only), turtle egg (living, with a draw only inside `canDestroyEgg`'s
  true branch), sculk sensor and shrieker (vibrations; skip).

**`checkInsideBlocks(list, collector)`** (lines 3303–3412): for each movement: `d =
to - from`; `budget = 16`; if it has an axis vector `vec` and `d.lengthSqr() > 0`:
for each axis of `Direction.axisStepOrder(vec)` with `d.axis != 0`: `next =
from.relative(axis.getPositive(), d.axis)`; `budget -= checkInsideBlocks(from, next,
collector, visited, budget)`; `from = next`. Else `budget -= checkInsideBlocks(m.from,
m.to, collector, visited, 16)`. If `budget <= 0`: `checkInsideBlocks(m.to, m.to,
collector, visited, 1)`. After all, `visited.clear()`.

`checkInsideBlocks(from, to, collector, visited, maxSteps)`: `box =
makeBoundingBox(to).deflate(9.999999747378752E-6)`; `long = from.distanceToSqr(to) >
Mth.square(0.9999900000002526)`; `BlockGetter.forEachBlockIntersectedBetween(from, to,
box, visitor)`; return `lastStep + 1`. The visitor (`lambda$checkInsideBlocks$0`,
lines 11888–12060), for `(pos, step)`: stop if the entity is not alive; stop if `step >=
maxSteps`; record `step`; air → continue; `inside = state.getEntityInsideCollisionShape(
level, pos, entity)`; `hitBlock = inside == Shapes.block() ||
collidedWithShapeMovingFrom(from, to, inside.move(pos).toAabbs())`;
`hitFluid = collidedWithFluid(state.getFluidState(), pos, from, to)`; if neither, or
`!visited.add(pos.asLong())`, continue; if `hitBlock`: `collector.advanceStep(step)`;
`state.entityInside(level, pos, entity, collector, long || box.intersects(pos))`;
`onInsideBlock(state)`; if `hitFluid`: `collector.advanceStep(step)`;
`state.getFluidState().entityInside(level, pos, entity, collector)`.

- `collidedWithShapeMovingFrom(from, to, boxes)` = `makeBoundingBox(from).
  collidedAlongVector(to - from, boxes)`: for each box, inflated by half the moving
  box's size plus `1e-7` on each axis, true if it contains the moving box's centre at
  `from` or at `to`, or the segment between them clips it (`AABB.clip`).
- `collidedWithFluid(fluid, pos, from, to)`: `aabb = fluid.getAABB(level, pos)` (null
  for no fluid; else `AABB(x, y, z, x + 1, (double)((float)y + height), z + 1)`
  with `height = FluidState.getHeight` (1 under the same fluid, else `amount/9f`) — note
  the float add); then as above with `[aabb]`.
- `getEntityInsideCollisionShape` is the full block (`Shapes.block()`) but for: end
  portal, tripwire, lava cauldron, powder snow, water cauldrons
  (`LayeredCauldronBlock`) (extract theirs).
- **`BlockGetter.forEachBlockIntersectedBetween(from, to, box, visitor)`**
  (BlockGetter.txt lines 393–499, with `addCollisionsAlongTravel` 500–847 and
  `getFurthestCorner` 848–931, `BlockPos.betweenCornersInDirection` and
  `betweenClosed`): if `(to - from).lengthSqr() < (double)Mth.square(1.0E-5F)` visit
  every cell of `BlockPos.betweenClosed(box)` with step 0. Otherwise visit the cells of
  the start box (`box.move(-delta)`) in `betweenCornersInDirection(…, delta)` order with
  step 0, remembering them; walk the box's furthest corner along `delta` cell by cell
  (`addCollisionsAlongTravel`, which visits the cells the swept box enters with step
  numbers 1, 2, … and returns the last, or a negative number when the visitor stopped);
  then the end box's cells not yet visited, with that number + 1. Port these three
  methods line by line; the step numbers decide which effects share a step.

**`InsideBlockEffectApplier.StepBasedCollector`**: `apply(type)` adds `type` to a set
for the current step; `runBefore/runAfter(type, consumer)` queue consumers.
`advanceStep(step)`: when the step differs from the last, `flushStep()`. `flushStep()`:
for each type in **`APPLY_ORDER` = FREEZE, CLEAR_FREEZE, FIRE_IGNITE, LAVA_IGNITE,
EXTINGUISH**: append its "before" consumers, then the type's effect if it was applied in
this step (once per step however many blocks applied it), then its "after" consumers.
`applyAndClear(entity)`: `flushStep()`, then run the queued list in order while the
entity is alive, clear, `lastStep = -1`. The effects: FREEZE (`setIsInPowderSnow(true)`,
freezing for things that can freeze), CLEAR_FREEZE (`clearFreeze`), FIRE_IGNITE
(`BaseFireBlock.fireIgnite`), LAVA_IGNITE (`Entity.lavaIgnite`), EXTINGUISH
(`clearFire`).

Inside effects that act on items and falling blocks:
- **Fire and soul fire** (`BaseFireBlock.entityInside`): `apply(CLEAR_FREEZE)`,
  `apply(FIRE_IGNITE)`, `runAfter(FIRE_IGNITE, e -> e.hurt(inFire, fireDamage))`, fire
  `1.0f`, soul fire `2.0f`. `fireIgnite(e)`: unless fire-immune: if
  `remainingFireTicks < 0` add 1, else (a `ServerPlayer` only: `+ level.random.
  nextInt(1, 3)`); then if `remainingFireTicks >= 0`: `igniteForSeconds(8.0f)`
  (`igniteForTicks(Mth.floor(8.0f*20.0f))` = 160: raise to 160 if lower; `clearFreeze`).
- **Lava** (`LavaFluid.entityInside`): `apply(CLEAR_FREEZE)`, `apply(LAVA_IGNITE)`,
  `runAfter(LAVA_IGNITE, Entity::lavaHurt)`. `lavaIgnite`: unless fire-immune,
  `igniteForSeconds(15.0f)` (300 ticks). `lavaHurt`: unless fire-immune,
  `hurtServer(lava, 4.0f)`, and if that hurt and `shouldPlayLavaHurtSound()` (an item:
  `health <= 0 || tickCount % 10 == 0`) and not silent: `playSound(GENERIC_BURN, 0.4f,
  2.0f + random.nextFloat() * 0.4f)` (**the entity's random**).
- **Water** (`WaterFluid.entityInside`): `apply(EXTINGUISH)`. Water cauldrons: the
  same (`LayeredCauldronBlock`); lava cauldron: CLEAR_FREEZE and LAVA_IGNITE.
- **Cactus** (`CactusBlock.entityInside`): `entity.hurt(cactus, 1.0f)` at once (its
  inside shape is the whole cell, so an item touching its side is inside).
- **Cobweb** (`WebBlock`): `makeStuckInBlock(state, Vec3(0.25, 0.05000000074505806,
  0.25))` for anything (0.5, 0.25, 0.5 for a living entity with Weaving).
  `makeStuckInBlock`: `resetFallDistance()`; `stuckSpeedMultiplier = v`.
- **Powder snow**: stuck `(0.9, 1.5, 0.9)` and FREEZE/EXTINGUISH for any entity
  (falling `PowderSnowBlock.entityInside` for details); items fall through it.
- **Honey block sides** (`HoneyBlock.entityInside` → `isSlidingDown`, any entity;
  the effects only for living, minecarts, TNT, boats): not on ground; `y <= pos.y +
  0.9375 - 1e-7`; `v.y / 0.9800000190734863 + 0.08 < -0.08`; and `|pos.x + 0.5 - x| +
  1e-7 > 0.4375 + (double)(bbWidth / 2.0F)` or the same in z. Then
  (`doSlideMovement`): if `v.y / 0.98… + 0.08 < -0.13`: `k = -0.05 / (that)`; `v =
  (v.x * k, (-0.05 - 0.08) * 0.9800000190734863, v.z * k)`; else `v = (v.x, (-0.05 -
  0.08) * 0.98…, v.z)`; `resetFallDistance()`. Rare for items; port or name.
- **Bubble columns**: Clustine forms none (fluids.rs): name it.
- **Hoppers, pressure plates, tripwire, detector rails**: N6. **Portals**: not in N4.
- Campfire, sweet berry bush, wither rose, magma: living only.

### 1.10 `getDeltaMovement` handling

`Vec3` is immutable; `setDeltaMovement` replaces it, `addDeltaMovement(v)` adds. The
operations used: `add`, `subtract`, `multiply(x,y,z)` (component-wise), `scale(d)`,
`lengthSqr` (`x*x + y*y + z*z`), `length` (`Math.sqrt`), `horizontalDistanceSqr`
(`x*x + z*z`), `distanceToSqr(o)` (`(o.x-x)² + …`, computed as `dx*dx + dy*dy +
dz*dz` with `dx = o.x - x`), `normalize()` (`d = Math.sqrt(x*x + y*y + z*z)`; `d <
9.999999747378752E-6` → ZERO, else `(x/d, y/d, z/d)`), `with(axis, v)`.

---------------------------------------------------------------------------------------

## 2. Gravity, drag and the item's numbers

- `getDefaultGravity()`: items `0.04` (`0x3fa47ae147ae147b`), falling blocks `0.04`;
  `getGravity()` = 0 if `NoGravity`, else the default. `applyGravity()`: if gravity
  != 0, `deltaMovement = deltaMovement.add(0, -gravity, 0)`.
- `getAirDrag()` = `0.98f` (`0x3f7ae148`; widened `0.9800000190734863`,
  `0x3fef5c2900000000`); neither type overrides it.
- `ItemEntity`: `DEFAULT_HEALTH = 5`, `LIFETIME = 6000`, `INFINITE_PICKUP_DELAY =
  32767`, `INFINITE_LIFETIME = -32768`, eye height `0.2125f`; `setDefaultPickUpDelay`
  = 10 (block drops, falling-block drops), the Q throw 40 (N2); `makeFakeItem` sets
  never and age 5999.
- Tracking (`EntityTypes`): item range 6 chunks, update interval 20; falling block
  range 10 chunks, interval 20; both `trackDeltas = true` (the builder's default).

---------------------------------------------------------------------------------------

## 3. `ItemEntity.tick` (ItemEntity.txt lines 207–558)

### 3.1 The whole tick, in order

```
if getItem().isEmpty(): discard(); return
super.tick()                         // Entity.tick → baseTick (3.2)
if pickupDelay > 0 && pickupDelay != 32767: pickupDelay--
xo, yo, zo = x, y, z                 // again (commonTick did it)
v0 = getDeltaMovement()
if isInWater() && getFluidHeight(WATER) > 0.10000000149011612:  setUnderwaterMovement()
elif isInLava() && getFluidHeight(LAVA) > 0.10000000149011612:  setUnderLavaMovement()
else: applyGravity()
// server only (the client always clears noPhysics):
noPhysics = !level.noCollision(this, getBoundingBox().deflate(1.0E-7))
if noPhysics: moveTowardsClosestSpace(x, (bb.minY + bb.maxY) / 2.0, z)
if !onGround() || getDeltaMovement().horizontalDistanceSqr() > 9.999999747378752E-6
      || (tickCount + getId()) % 4 == 0:
    move(SELF, getDeltaMovement())
    applyEffectsFromBlocks()
    f2 = getAirDrag()                // 0.98f
    f3 = f2
    if onGround(): f3 = f2 * level.getBlockState(getBlockPosBelowThatAffectsMyMovement()).getBlock().getFriction()   // float
    setDeltaMovement(getDeltaMovement().multiply((double)f3, (double)f2, (double)f3))
    if onGround() && getDeltaMovement().y < 0: setDeltaMovement(v.multiply(1.0, -0.5, 1.0))
else:
    applyEffectsFromBlocksForLastMovements()
movedBlock = floor(xo) != floor(x) || floor(yo) != floor(y) || floor(zo) != floor(z)
if tickCount % (movedBlock ? 2 : 40) == 0 && server && isMergable(): mergeWithNeighbours()
if age != -32768: age++
needsSync |= updateFluidInteraction()          // the second push (section 4)
if server && getDeltaMovement().subtract(v0).lengthSqr() > 0.01: needsSync = true
if server && age >= 6000: discard()
```

- `setUnderwaterMovement()` = `setFluidMovement(0.9900000095367432)` (0.99f widened,
  `0x3fefae1480000000`); `setUnderLavaMovement()` = `setFluidMovement(
  0.949999988079071)` (0.95f widened, `0x3fee666660000000`). `setFluidMovement(d)`:
  `(v.x * d, v.y + (v.y < 0.05999999865889549 ? (double)5.0E-4F : 0.0), v.z * d)`,
  with `5.0E-4F` widened = `0.0005000000237487257` (`0x3f40624de0000000`). **No
  gravity in a fluid**: the item rises at `5e-4` a tick until it is near the surface.
- `getFluidHeight`, `isInWater`, `isInLava` are as of the first fluid update in
  `baseTick` (`isInLava` is false on the first tick, `firstTick`).
- `level.noCollision(entity, box)`: no block collision (`BlockCollisions` with
  `joinIsNotEmpty`), no entity collision and no border collision.
- **`moveTowardsClosestSpace(x, y, z)`** (Entity.txt 8213–8348): `p =
  BlockPos.containing(x, y, z)`; `frac = (x - p.x, y - p.y, z - p.z)`; best = UP,
  `bestD = Double.MAX_VALUE`; for `dir` in **NORTH, SOUTH, WEST, EAST, UP**: if the
  block at `p + dir` is not `isCollisionShapeFullBlock`: `c = frac.get(dir.axis)`; `d =
  dir positive ? 1.0 - c : c`; if `d < bestD`: best = dir, `bestD = d`. Then `speed =
  random.nextFloat() * 0.2f + 0.1f` (**the entity's random, one float draw**, float
  arithmetic); `step = (float)best.axisDirection.step`; `v = deltaMovement.scale(
  0.75)`; set the best axis's component to `(double)(step * speed)` (float product),
  the other two from `v`. (The repo has `collision_is_full_block`.)
- The `y *= -0.5` line almost never fires (finding 12).
- **Java details**: `tickCount + getId()` wraps; `% 4` may be negative, so `== 0` only
  when divisible. Clustine's `game_id()` is the key's low 32 bits as `i32` and may be
  negative: use it as the game would.

### 3.2 `Entity.baseTick` as items meet it (Entity.txt 963–1106)

`computeSpeed` (last known speed; skip unless used); `inBlockState = null`; riding,
boarding cooldown, portals, sprint particles, powder snow flags (skip for items);
`wasEyeInWater = isEyeInFluid(WATER)`; **`updateFluidInteraction()`** (section 4,
its result discarded); `updateSwimming` (players); on the server: if
`remainingFireTicks > 0`: if `fireImmune()` `clearFire()` (`setRemainingFireTicks(
min(0, ticks))`); else { if `remainingFireTicks % 20 == 0 && !isInLava()`:
`hurtServer(level, onFire, 1.0f)`; `setRemainingFireTicks(remainingFireTicks - 1)` }.
If `isInLava()`: `fallDistance *= 0.5`. `checkBelowWorld()`: `y < minY - 64` →
`onBelowWorld()` → `discard()` (the item tick then goes on with a removed entity: stop
there in Clustine). Server: shared flag ON_FIRE (data 0, bit 0) =
`remainingFireTicks > 0` (or visual fire). `firstTick = false`.

### 3.3 Damage: `hurtServer`, fire immunity, burning

`ItemEntity.hurtServer(level, source, amount)`: false if `isInvulnerableToBase(source)`
(e.g. `/kill`-type sources and invulnerable entities); false if the game rule
`mobGriefing` is off and the source's entity is a `Mob`; false if
`!getItem().canBeHurtBy(source)` (the stack's `minecraft:damage_resistant` component,
e.g. netherite: `{"types": "#minecraft:is_fire"}`); then `markHurt()` (**sets
`syncVelocity`**, so the edge sends `SetEntityMotion` that tick, section 9); `health =
(int)((float)health - amount)`; game event ENTITY_DAMAGE (skip); if `health <= 0`:
`getItem().onDestroyed(this)` (bundles and shulker boxes spill their contents: later,
with components) and `discard()`. True.

`fireImmune()` = `!getItem().canBeHurtBy(inFire) || super.fireImmune()` (the entity
type: items are not).

What hurts an item in N4: **cactus** 1 per tick inside (5 ticks); **fire** `inFire`
1 (soul fire 2) per fire block per step, and **burning** `onFire` 1 every 20 ticks
while `remainingFireTicks > 0` and not in lava; **lava** 4 per tick (two ticks) plus
300 ticks of fire. Elsewhere: explosions (`ignoreExplosion` is true unless the
explosion `shouldAffectBlocklikeEntities`; N6), lightning (`thunderHit`: fire and 5
damage; later), the void (`discard` below `minY - 64`). Not by: falling anvils and
stalactites (living only), campfires, magma, berry bushes.

### 3.4 Merging

- `isMergable()`: alive, `pickupDelay != 32767`, `age != -32768`, `age < 6000`, and
  `count < getMaxStackSize()` (the stack's `minecraft:max_stack_size`).
- `mergeWithNeighbours()`: return unless mergable; candidates =
  `level.getEntitiesOfClass(ItemEntity, getBoundingBox().inflate(0.5, 0.0, 0.5), e ->
  e != this && e.isMergable())`, in the level's lookup order (ADR-0028: chunk, section,
  key); for each, if still mergable, `tryToMerge(other)`, and stop once this one is
  removed.
- `tryToMerge(other)`: both `target` (the "Owner" UUID, not the thrower) equal
  (`Objects.equals`, both null in N4) and `areMergable(this.stack, other.stack)`
  (`other.count + this.count <= other.getMaxStackSize()` and
  `ItemStack.isSameItemSameComponents`), else return. If `other.count < this.count`:
  `merge(this, thisStack, other, otherStack)`, else `merge(other, otherStack, this,
  thisStack)` (the larger absorbs the smaller; on equal counts the other absorbs this).
- `merge(target, tStack, source, sStack)`: `moved = min(min(tStack.max, 64) -
  tStack.count, sStack.count)`; `target.setItem(tStack.copyWithCount(tStack.count +
  moved))` (data dirty: `SetEntityData`); `sStack.shrink(moved)`;
  `target.pickupDelay = max(target.pickupDelay, source.pickupDelay)`;
  `target.age = min(target.age, source.age)`; if `sStack` empty: `source.discard()`
  (always, since the sum fits).
- An item absorbed by a merge is removed: the edge removes it (no `TakeItemEntity`).

### 3.5 What an item stores, and a resting item

The game saves `Health` (short), `Age` (short), `PickupDelay` (short), `Thrower`,
`Owner`, `Item`, and the entity's `Motion`, `OnGround`, `fall_distance`, `Fire`, …
Clustine's `EntityData::Item` needs `health` and, on the entity, `fall_distance` (f64)
and `remaining_fire_ticks` (i32); `target` can wait (only `/give` sets it). In memory
only (the game does not save them either; reset at take-in): `tick_count` (or derived),
`first_tick`, `main_supporting_block`, `on_ground_no_blocks`, the fluid trackers, the
last tick's movements, `stuck_speed_multiplier`, `no_physics`, and the per-tick flags
the edge needs (section 9).

**Proposal for "at rest commits nothing"** (finding 5): apply the motion of a tick in
memory always, but put `Motion` (and the counters) into the commit only in a tick that
also changes the position, on-ground, the data or the fire, or at a checkpoint (which
writes the chunk whole). A crash then restores an item as of its last record, which is
exactly the game's state for an entity whose chunk was unloaded since then (as
ADR-0028 names for counters). Copies may read a stale motion of a resting item, which
nothing reads. To be agreed in the plan.

---------------------------------------------------------------------------------------

## 4. Water and lava: `EntityFluidInteraction`

`Entity.updateFluidInteraction()` (Entity.txt 4284–4350):

```
found = fluidInteraction.update(this, !isPushedByFluid())   // items: isPushedByFluid() = true → currents on
inWater = fluidInteraction.isInFluid(WATER); inLava = fluidInteraction.isInFluid(LAVA)
if inWater: resetFallDistance(); if !wasTouchingWater && !firstTick: doWaterSplashEffect()
wasTouchingWater = inWater
if isPushedByFluid():
    if inWater: fluidInteraction.applyCurrentTo(WATER, this, 0.014)
    if inLava:  fluidInteraction.applyCurrentTo(LAVA, this, fastLava ? 0.007 : 0.0023333333333333335)
return found
```

The overworld's `fast_lava` is false: `0.0023333333333333335` (`0x3f631d5acb6f4651`);
water `0.014` (`0x3f8cac083126e979`). `doWaterSplashEffect` is particles and a sound
(an item's splash: skip or send the sound later).

**`EntityFluidInteraction.update(entity, ignoreCurrent)`** (EntityFluidInteraction.txt
lines 48–210): the entity keeps a list of `Tracker`s (one per fluid *type*, so still
and flowing water are two) and one `CurrentAccumulator` per fluid tag of
`FLUIDS_WITH_CURRENT` (water, lava).

```
trackers.removeIf(t -> t.reset())        // reset: true (remove) if height == 0 && !eyesInside; else height = 0, eyesInside = false
for acc in accumulators: acc.reset()     // height 0, current ZERO, count 0
box = entity.getFluidInteractionBox()    // bb.deflate(0.001) (a vehicle may change it)
x0 = floor(box.minX); y0 = floor(box.minY); z0 = floor(box.minZ)
x1 = ceil(box.maxX) - 1; y1 = ceil(box.maxY) - 1; z1 = ceil(box.maxZ) - 1
if !hasFluidAndLoaded(level, x0 - 1, y0, z0 - 1, x1 + 1, y1, z1 + 1): return false
minY = entity.getBoundingBox().minY      // the undeflated box
lastType = null; tracker = null; acc = null
for x in x0..=x1: for y in y0..=y1: for z in z0..=z1:     // x outermost, z innermost
    fluid = level.getFluidState(x, y, z); if fluid.isEmpty(): continue
    top = (double)y + (double)fluid.getHeight(level, pos)
    if top < box.minY: continue
    type = fluid.typeHolder()
    if type != lastType:
        lastType = type; tracker = getOrCreateTrackerFor(type)
        if !ignoreCurrent: acc = getCurrentAccumulatorFor(type)   // the accumulator of the first tag the type is in
    (eyes: if x == entity.blockX && z == entity.blockZ && eyeY >= y && eyeY <= y + getHeightForCamera: tracker.eyesInside = true)
    tracker.height = max(top - minY, tracker.height)
    if acc != null:
        flow = fluid.getFlow(level, pos)
        acc.height = max(tracker.height, acc.height)               // running maxima, order-dependent
        if acc.height < 0.4: flow = flow.scale(acc.height)
        acc.accumulate(flow)                                       // current += flow; count++
return lastType != null
```

`hasFluidAndLoaded`: every chunk of sections `x0-1 … x1+1` × `z0-1 … z1+1` must be
loaded at FULL (else false at once); then true iff any section in `y0 … y1` (by section
Y) of those chunks has fluid. Only the "loaded" half changes a result. The eye parts
are only for `isEyeInFluid` (not used by items or falling blocks).

`getFluidHeight(tag)`: the largest tracker height among trackers whose type is in the
tag (strictly greater replaces; starts at 0). `isInFluid(tag)`: that `> 0`.

**`CurrentAccumulator.applyTo(entity, scale)`**:

```
if count == 0 || current.lengthSqr() < 9.999999747378752E-6: return
c = entity is Player ? current.scale(1.0 / count) : current.normalize()
v = entity.getDeltaMovement()
c = c.scale(scale)
if |v.x| < 0.003 && |v.z| < 0.003 && c.length() < 0.0045000000000000005:
    c = c.normalize().scale(0.0045000000000000005)
entity.addDeltaMovement(c)
```

(`0.0045000000000000005` = `0x3f726e978d4fdf3c`, `0.003` = `0x3f689374bc6a7efa`.)

**`FlowingFluid.getFlow(level, pos, fluid)`** (FlowingFluid.txt, `getFlow`; the empty
fluid returns ZERO):

```
dx = 0.0; dz = 0.0
for dir in NORTH, EAST, SOUTH, WEST:                // Direction.Plane.HORIZONTAL
    n = pos + dir; nf = level.getFluidState(n)
    if !affectsFlow(nf): continue                   // nf.isEmpty() || nf.getType().isSame(this)
    h = nf.getOwnHeight(); diff = 0.0f
    if h == 0.0f:
        if !level.getBlockState(n) in #minecraft:blocks_fluid_flow:
            bf = level.getFluidState(n.below())
            if affectsFlow(bf):
                h = bf.getOwnHeight()
                if h > 0.0f: diff = fluid.getOwnHeight() - (h - 0.8888889f)
    elif h > 0.0f: diff = fluid.getOwnHeight() - h
    if diff != 0.0f:
        dx += (double)((float)dir.stepX * diff)
        dz += (double)((float)dir.stepZ * diff)
flow = Vec3(dx, 0.0, dz)
if fluid.getValue(FALLING):
    for dir in NORTH, EAST, SOUTH, WEST:
        n = pos + dir
        if isSolidFace(level, n, dir) || isSolidFace(level, n.above(), dir):
            flow = flow.normalize().add(0.0, -6.0, 0.0); break
return flow.normalize()
```

`getOwnHeight()` = `amount / 9.0f` (the repo's `own_height`); `0.8888889f` =
`0x3f638e39`. `isSolidFace(level, p, dir)`: false if the fluid at `p` is the same
fluid; true if `dir == UP`; false for an `IceBlock`; else
`state.isFaceSturdy(level, p, dir)` (the repo's `is_face_sturdy(face, Full)`). Water
and lava are both `FlowingFluid`; `isSame` compares the kind (still and flowing water
are the same). The tag `minecraft:blocks_fluid_flow` is in the repo's tags.

**What the sim lacks for this**: a `flow` function (above) beside `fluid::height`; the
section "has fluid" test may be skipped (only the loaded test matters); the tag lookup;
everything else (amounts, falling, sturdy faces) is there.

---------------------------------------------------------------------------------------

## 5. Pick-up (`Player.aiStep` → `ItemEntity.playerTouch`)

`Player.aiStep` (Player.txt 773–902), after the living entity's own `aiStep`: if
`getHealth() > 0 && !isSpectator()`: `box = isPassenger() && !vehicle.isRemoved() ?
getBoundingBox().minmax(vehicle.getBoundingBox()).inflate(1.0, 0.0, 1.0) :
getBoundingBox().inflate(1.0, 0.5, 1.0)`; `level.getEntities(this, box)` (every entity
but the player whose box intersects); each that is not an experience orb and not
removed is touched (`entity.playerTouch(player)`) in lookup order; experience orbs: one
chosen at random from the player's random (later). It runs in `ServerPlayer.doTick`,
from the connection's tick after the levels' (finding 7). The player's box: width 0.6,
height 1.8 standing (1.5 crouching).

`ItemEntity.playerTouch(player)` (server only):

```
stack = getItem(); item = stack.getItem(); count = stack.getCount()
if pickupDelay == 0 && (target == null || target.equals(player.getUUID())) && player.getInventory().add(stack):
    player.take(this, count)                    // TakeItemEntity, before the removal
    if stack.isEmpty(): discard(); stack.setCount(count)
    player.awardStat(ITEM_PICKED_UP, count); player.onItemPickup(this)   // stats, advancements: skip
```

`LivingEntity.take(entity, count)`: if the entity is not removed and is an item, an
arrow or an orb: `chunkSource.sendToTrackingPlayers(entity, TakeItemEntity(entityId,
playerId, count))` (to everyone tracking the item, the picker among them). Wire:
`TAKE_ITEM_ENTITY` (127): `varint item id, varint collector id, varint amount`, ids
translated by the edge (ADR-0028 section 1). Then `RemoveEntities` when the item is
gone. The client animates and plays the sound (finding 9).

**`Inventory.add(stack)`** = `add(-1, stack)` (Inventory.txt 573–714). Slots: `items`
0..35 (0–8 the hotbar), 36–39 armour, 40 the off hand (Clustine's 41 slots match).

```
if stack.isEmpty(): return false
if !stack.isDamaged():
    do:
        i = stack.getCount()
        stack.setCount(addResource(stack))            // slot -1
    while !stack.isEmpty() && stack.getCount() < i     // again while the last pass made progress
    if stack.getCount() == i && player.hasInfiniteMaterials(): stack.setCount(0); return true
    return stack.getCount() < i                       // compares with the last pass only
else:                                                  // damaged tools: one free slot or nothing
    slot = getFreeSlot()
    if slot >= 0: items[slot] = stack.copyAndClear(); items[slot].setPopTime(5); return true
    if player.hasInfiniteMaterials(): stack.setCount(0); return true
    return false
```

`addResource(stack)`: `slot = getSlotWithRemainingSpace(stack)`, else
`getFreeSlot()`; none → return the count. `getSlotWithRemainingSpace`: the **selected
slot** first, then **slot 40 (off hand)**, then **0..35 in order**; a slot qualifies if
`hasRemainingSpaceForItem(slotStack, stack)`: not empty, same item and components,
`slotStack.isStackable()` (max stack > 1 and not a damaged damageable), and
`slotStack.count < getMaxStackSize(slotStack)` (`min(99, stack's max)`).
`getFreeSlot()`: the first empty of 0..35 (so the hotbar first; never the off hand or
armour). `addResource(slot, stack)`: an empty slot gets `stack.copyWithCount(0)`; `n =
min(stack.count, max - slotStack.count)`; `n == 0` → return the count; else grow the
slot by `n`, `setPopTime(5)` (the client's bounce animation: send the slot), return
`count - n`.

So a creative player picks up everything and what does not fit is destroyed; a survival
player's partial fit is finding 8. The region tells the edge of changed slots
(`RegionEvent::InventorySlot`, N2) for `SetPlayerInventory`.

---------------------------------------------------------------------------------------

## 6. When a block starts to fall

`FallingBlock` (sand, red sand, gravel, concrete powder, anvils, dragon egg; the class
`ColoredFallingBlock`/`SandBlock`): `onPlace` and `updateShape` schedule a tick after
`getDelayAfterPlace()` (2; the dragon egg 5). `FallingBlock.tick(state, level, pos,
random)`: if `isFree(level.getBlockState(pos.below()))` and `pos.y >= level.getMinY()`:
`e = FallingBlockEntity.fall(level, pos, state)`; `falling(e)` (anvils:
`e.setHurtsEntities(2.0f, 40)`; others nothing). `isFree(s)` = air, `#minecraft:fire`,
`liquid()`, or `canBeReplaced()` (the state's flag).

`FallingBlockEntity.fall(level, pos, state)`: a new entity at `(pos.x + 0.5, pos.y,
pos.z + 0.5)` with `state` (with `waterlogged=false` if it has the property);
`blocksBuilding = true`; motion ZERO; `xo/yo/zo` = that position; `startPos =
blockPosition()` (synched data, index 8, `BLOCK_POS`); then
`level.setBlockAndUpdate(pos, state.getFluidState().createLegacyBlock())` (the fluid it
held, or air; all updates); then `addFreshEntity(e)`. Its own random draws only the
UUID (two longs). Defaults: `time 0`, `dropItem true`, `cancelDrop false`,
`hurtEntities false`, `fallDamageMax 40`, `fallDamagePerDistance 0`.

Other falling blocks:
- **Suspicious sand and gravel** (`BrushableBlock.tick`): `checkReset` of its block
  entity, then the same `isFree` test, `fall`, and `disableDrop()` (`cancelDrop =
  true`: it never lands and never drops). `onBrokenAfterFall`: level event 2001
  (block break particles of the state) at the box's centre, game event BLOCK_DESTROY.
- **Scaffolding** (`ScaffoldingBlock.tick`): works out `distance`/`bottom`; if the
  new distance is 7: if the old state's distance was 7, `fall` (a falling entity),
  else `level.destroyBlock(pos, true)` (drops); otherwise `setBlockAndUpdate` if
  changed.
- **Pointed dripstone and the sulfur spike** (`SpeleothemBlock.tick`): a stalagmite that
  cannot survive is `destroyBlock(pos, true)`; a stalactite spawns the falling
  stalactite (`spawnFallingStalactite`): from `pos` downwards while the state is a
  stalactite, `fall` each; the tip's entity `setHurtsEntities(max(1 + pos.y - y, 6) *
  1.0f, 40)` (its damage: living entities, left out with anvils'). Landing: a
  stalactite cannot survive on the floor (it needs support above), so it drops as its
  item and `onBrokenAfterFall` plays `getStalactiteLandingSound()` (level event; 1045
  for dripstone, to read for the spike). The repo destroys a stalactite instead today
  (`ticks.rs` comment): N4 ports the fall.
- **Concrete powder**: section 7, landing in water.
- **Dragon egg**: falls with delay 5 (its teleport is separate).
- **Powder snow** is not a falling block (it only references the entity for its shape).

The repo's `Region::fall` (`crates/clustine-sim/src/region/ticks.rs:558`) lands the
block at once; N4 replaces it with the entity.

---------------------------------------------------------------------------------------

## 7. `FallingBlockEntity.tick` (FallingBlockEntity.txt 207–642)

```
if blockState.isAir(): discard(); return
block = blockState.getBlock()
time++
applyGravity()                                       // -0.04
move(SELF, getDeltaMovement())
applyEffectsFromBlocks()
handlePortal()                                       // not in N4
if server && (isAlive() || forceTickAfterTeleportToDuplicate):
    pos = blockPosition()
    concrete = block instanceof ConcretePowderBlock
    inWater = concrete && level.getFluidState(pos).is(#water)
    if concrete && getDeltaMovement().lengthSqr() > 1.0:
        hit = level.clip(ClipContext(Vec3(xo, yo, zo), position(), Block.COLLIDER, Fluid.SOURCE_ONLY, this))
        if hit.type != MISS && level.getFluidState(hit.pos).is(#water): pos = hit.pos; inWater = true
    if onGround() || inWater:
        here = level.getBlockState(pos)
        setDeltaMovement(getDeltaMovement().multiply(0.7, -0.5, 0.7))
        if !here.is(MOVING_PISTON):
            if !cancelDrop:
                replaceable = here.canBeReplaced(DirectionalPlaceContext(level, pos, DOWN, ItemStack.EMPTY, UP))
                belowFree = FallingBlock.isFree(level.getBlockState(pos.below())) && (!concrete || !inWater)
                survives = blockState.canSurvive(level, pos) && !belowFree
                if replaceable && survives:
                    if blockState has WATERLOGGED && level.getFluidState(pos).is(Fluids.WATER):   // still water exactly
                        blockState = blockState.setValue(WATERLOGGED, true)
                    if level.setBlockAndUpdate(pos, blockState):
                        send to trackers of this entity: BlockUpdate(pos, level state at pos), then AddTransientBlock(pos, level state at pos)
                        discard()
                        if block instanceof Fallable: block.onLand(level, pos, blockState, here, this)
                        (block entity data: merged into the new block entity if any; not in N4)
                    elif dropItem && gamerule entityDrops:
                        discard(); callOnBrokenAfterFall(block, pos); spawnAtLocation(level, block)
                else:
                    discard()
                    if dropItem && entityDrops: callOnBrokenAfterFall(block, pos); spawnAtLocation(level, block)
            else:
                discard(); callOnBrokenAfterFall(block, pos)
        // on a moving piston nothing more: the entity stays, and lands next tick
    elif (time > 100 && (pos.y <= level.getMinY() || pos.y > level.getMaxY())) || time > 600:
        if dropItem && entityDrops: spawnAtLocation(level, block)
        discard()
setDeltaMovement(getDeltaMovement().scale((double)getAirDrag()))   // always, after all of the above
setRequiresPrecisePosition(horizontalCollision || verticalCollision)  // read by the edge, section 9
```

- `0.7` and `-0.5` are doubles (`0x3fe6666666666666`, `0xbfe0000000000000`).
- `canBeReplaced(DirectionalPlaceContext(level, pos, DOWN, EMPTY, UP))`: with an empty
  hand `BlockBehaviour.canBeReplaced(state, ctx)` is the state's own flag, except the
  overrides (snow layers: only `layers == 1`; slabs, sea pickles, candles, turtle eggs,
  vines, glow lichen and other multiface blocks, scaffolding, growing plant bodies,
  each with its own rule, all false or the flag for an empty hand). The repo has
  `crates/clustine-sim/src/placement/replace.rs` (`can_be_replaced(state, &Context)`);
  give it an empty item and `DOWN` as the clicked face. (`DirectionalPlaceContext.
  replacingClickedOnBlock()` is `canPlace()`, the state's `canBeReplaced(this)`; the
  overrides ask it only when the hand holds their own item, so never here.)
- `canSurvive`: the repo's `survive.rs`.
- `ConcretePowderBlock.onLand(level, pos, state, replaced, entity)`: if
  `shouldSolidify(level, pos, replaced)` → `setBlockAndUpdate(pos, concrete)` (the
  repo's `powder_hardens`). Anvil `onLand` level event 1031, `onBrokenAfterFall` 1029
  (sounds; not silent); dripstone's `onBrokenAfterFall` its landing sound.
- `spawnAtLocation(level, block)` → `spawnAtLocation(level, new ItemStack(block), 0f)`
  → if the stack is empty (a block with no item) nothing; else `new ItemEntity(level,
  x, y + 0.0, z, stack)` (the random motion constructor, section 8.4) with
  `setDefaultPickUpDelay()` (10), `addFreshEntity`.
- `hurtServer` of a falling block: never hurts (false; `markHurt` unless invulnerable).
  `isAttackable` false; `isPickable` while not removed.
- The `clip` for fast concrete powder (more than one block a tick, i.e. after a fall of
  about eight blocks) needs a block traversal with collision shapes and source fluids
  (the bucket's clip in `placement/bucket.rs` is the model).

Falling blocks have no `baseTick` (finding 4): they fall through water at full speed,
are not pushed, do not burn, and are not checked against `minY - 64` but by the `time >
100` rule.

---------------------------------------------------------------------------------------

## 8. Drops from blocks

### 8.1 Who drops

`Block.dropResources(state, level, pos[, blockEntity[, entity, tool]])`: on the server,
`getDrops(…)` and `popResource(level, pos, stack)` for each in order, then
`state.spawnAfterBreak(level, pos, tool, true)` (experience from ores, silverfish from
infested blocks: not in N4; name them). `Level.destroyBlock(pos, drop=true)` calls
`dropResources(state, level, pos, blockEntity, null, ItemStack.EMPTY)`. Rule breaks
that drop: support lost (`updateShape` returning air via `destroyBlock`), cactus and
sugar cane and kelp ticks, pistons destroying `POPPED`/`DESTROY` blocks, water and lava
washing away (`FlowingFluid.beforeDestroyingBlock` → `dropResources(state, level, pos,
blockEntity)`; lava's version plays the fizz instead, no drops), leaf decay
(`LeavesBlock.randomTick` → `dropResources(state, level, pos)`), scaffolding. The repo's
`destroy_block` takes `_drops` and ignores it; `fluids.rs` names "a block a fluid washes
away drops nothing". A creative player's break drops nothing
(`ServerPlayerGameMode.destroyBlock`); a survival player's goes through
`Block.playerDestroy` with the tool (Clustine's stacks have no enchantments: silk touch
and fortune never apply).

`getDrops(state, level, pos, blockEntity)`: `LootParams` with `ORIGIN =
Vec3.atCenterOf(pos)`, `TOOL = ItemStack.EMPTY`, `BLOCK_ENTITY` (optional), and the
`BLOCK_STATE` of the state; `state.getDrops(builder)` runs the block's loot table
(none → no drops; blocks with no table: air, fire, water, …).

### 8.2 The tables (generated/data/minecraft/loot_table/blocks: 1201 files)

Census of the 1201 files (type `minecraft:block` each, `random_sequence` each):
- 14 have no pools (budding amethyst, cake, fire, frogspawn, frosted ice, nether
  portal, powder snow, reinforced deepslate, soul fire, spawner, suspicious sand and
  gravel, trial spawner, vault).
- Every pool has `rolls: 1`, no `bonus_rolls`, exactly one entry; no weights.
- Entries: `minecraft:item` (1346), `minecraft:alternatives` (82), `minecraft:dynamic`
  (1: the decorated pot's sherds).
- Conditions (as `"condition"`, an object with `"type"`, or a string naming
  `data/minecraft/predicate/<path>.json`): `survives_explosion` 914, `match_block` 294
  (`blocks` and `state` with string values), `any_of` 38, `table_bonus` 35, `all_of`
  29, `inverted` 16, `random_chance` 7, `location_check` 4 (large ferns and tall
  grass: `offsetY` ±1 and a block predicate), `entity_properties` 2, `match_tool` 1;
  references `minecraft:tool/can_silk_touch` 162 (`match_tool` with silk touch ≥ 1)
  and `minecraft:tool/can_shear` 52 (`match_tool` with `items: minecraft:shears`).
- Modifiers (`"modifier"`, an object or a list; on an entry or on the whole table):
  `set_count` 274 (constant, `uniform` 33, `binomial` 18; `add`), `explosion_decay`
  190, `copy_components` 71 (containers, banners, heads, beehives, shulker boxes, the
  decorated pot…), `apply_bonus` 32 (`ore_drops` 17, `uniform_bonus_count` with
  `bonusMultiplier` 1 (9) or 2 (2), `binomial_with_bonus_count` extra 3 p 0.5714286
  (4)), `copy_state` 10, `limit_count` 6.
- `table_bonus` chance lists: `(0.05, 0.0625, 0.083333336, 0.1)` and the like (six
  lists; gravel's flint `(0.1, 0.14285715, 0.25, 1.0)`).

### 8.3 Evaluation (a block broken by a rule: no tool, no entity, no explosion)

`LootTable.getRandomItems(params, consumer)` → `getRandomItemsRaw(context,
createStackSplitter(level, consumer))`: the table's modifier decorates the consumer;
pools in order. `LootPool.addRandomItems`: the pool's condition; if it passes, `rolls`
times `addRandomItem`: expand the entries (each entry's condition; `alternatives`: the
first child whose expansion succeeds); none → nothing; one → `createItemStack` (no
draw); several → `nextInt(total weight)` (never for blocks). An item entry: its
modifiers in order (each conditional modifier tests its own condition first), then
the table's. The stack splitter drops items not enabled and splits a stack above its
max into max-sized stacks, in order.

The conditions and modifiers with the draws they take (`r` = the context's random):
- `survives_explosion`: with no `EXPLOSION_RADIUS` param **true, no draw**; with one:
  `r.nextFloat() <= 1.0f / radius` (N6).
- `explosion_decay`: no radius → nothing; with one: per item of the count,
  `r.nextFloat() <= 1/radius` keeps it.
- `match_tool` (both predicate files): the tool is EMPTY → **false**. So every
  silk-touch and shears branch fails for rule drops; the `alternatives` fall through.
- `table_bonus`: `level` of the enchantment on the tool (0 for EMPTY); `chance =
  chances[min(level, len - 1)]`; `r.nextFloat() < chance`.
- `random_chance`: `r.nextFloat() < chance` (a number provider; constant here).
- `match_block`: the state is the block and each named property's value (as its
  string) equals.
- `location_check` with `offsetY`: the block at `ORIGIN + (0, offsetY, 0)` (from the
  centre, floored) matches the block predicate.
- `entity_properties` `{"entity": "this", "predicate": {}}` (the chorus flower and
  one more): with no `THIS_ENTITY` the predicate is present and
  `EntityPredicate.matches(level, pos, null)` returns false at once → **false**: a
  chorus flower broken by a rule drops nothing.
- `any_of`, `all_of` short-circuit in order; `inverted`.
- `set_count`: `stack.setCount((add ? stack.count : 0) + count.getInt(context))`, the
  count an **int** provider in 26.3 (`providers/number/ints/`): a constant (no draw);
  `uniform` = `Mth.nextInt(r, min, max)` (`min >= max ? min : r.nextInt(max - min + 1)
  + min`, one draw); `binomial` (`ints/BinomialDistributionGenerator`) = `n` trials of
  `r.nextFloat() < p`. `setCount` does not clamp; the stack splitter splits above the
  item's max stack.
- `apply_bonus` with level 0: `ore_drops` nothing (no draw); `uniform_bonus_count`:
  `count + r.nextInt(0 * multiplier + 1)` (**one draw** of `nextInt(1)`);
  `binomial_with_bonus_count`: `extra + 0` = 3 trials, each `r.nextFloat() < p`
  adds one (**three draws**).
- `limit_count`: clamp.
- `copy_components`, `copy_state`, `dynamic`: need item components (custom names,
  container contents, honey level, pot sherds). Clustine's `ItemStack` is `{item,
  count}`: drop the plain item and name the difference. (A container's *contents*
  spill separately when it is removed, `Containers.dropContents`: not loot.)

### 8.4 `Block.popResource(level, pos, stack)` and the item it makes

```
h = (double)EntityTypes.ITEM.getHeight() / 2.0         // 0.125
x = pos.x + 0.5 + Mth.nextDouble(levelRandom, -0.25, 0.25)
y = pos.y + 0.5 + Mth.nextDouble(levelRandom, -0.25, 0.25) - h
z = pos.z + 0.5 + Mth.nextDouble(levelRandom, -0.25, 0.25)
if server && !stack.isEmpty() && gamerule blockDrops:
    e = new ItemEntity(level, x, y, z, stack)
    e.setDefaultPickUpDelay()                            // 10
    level.addFreshEntity(e)
```

`Mth.nextDouble(r, min, max)` = `min >= max ? min : r.nextDouble() * (max - min) + min`.
The three position draws come **before** the entity exists (the level's random; they
are drawn even when the stack is empty or drops are off, which skip only the entity).

`popResourceFromFace(level, pos, face, stack)` (shearing, some interactions): on the
face's axis `pos + 0.5 + step * (0.5 + w)` (`w` = width/2 for x and z, h for y) with no
draw, on the others `nextDouble(-0.25, 0.25)`; `y` minus `h`; motion on the face's axis
`step * 0.1` (y: `step * 0.1 + 0.1`), on the others `nextDouble(-0.1, 0.1)` (y:
`nextDouble(0, 0.1)`); the explicit-motion constructor (no motion draws).

**`new ItemEntity(level, x, y, z, stack)`**, the entity's own random, in order:
`Entity.<init>`: `uuid = Mth.createInsecureUUID(random)` (**two `nextLong`**); then
`bobOffs = random.nextFloat() * 3.1415927f * 2.0f` (**float**, the client's bob);
`setYRot(random.nextFloat() * 360.0f)` (**float**); `setPos`; `setItem`;
`setDeltaMovement(random.nextDouble() * 0.2 - 0.1, 0.2, random.nextDouble() * 0.2 -
0.1)` (**two `nextDouble`**, the second for z; `y` is the constant `0.2`). The
explicit-motion constructor `(level, x, y, z, stack, dx, dy, dz)` draws only the first
four. (N2's throw draws those four as draws 0 to 3 of the item's stream: the block drop
continues with draws 4 and 5.)

### 8.5 Streams for Clustine (proposal)

ADR-0028 section 4 gives an entity's own draws a stream by (world seed, key, tick,
index): the UUID, bob, yaw and motion of a new item come from the new key's stream at
its birth tick. The level's random of `popResource` and the loot table's random
sequence have no stateless equal; draw both, in the game's order (loot first, then for
each stack its three position draws), from one stream of the block (world seed, the
block's position, the tick, a purpose tag), the same way random ticks have
`Random::of_section`. A named difference: distributions are the game's, values are not.

---------------------------------------------------------------------------------------

## 9. What the server sends: `ServerEntity` (for the edge)

Per tracked entity, each server tick, `ServerEntity.sendChanges()` (ServerEntity.txt
114–559). The edge plays the part, from what the region reports; state kept per entity
(not per viewer): `tickCount` (from 0 when the entity starts being tracked),
`teleportDelay`, `positionCodec` base (a `VecDeltaCodec`, base set at spawn to the
spawn position), `lastSentMovement`, `lastSentYRot/XRot/YHeadRot` (`Mth.packDegrees`),
`wasOnGround`, `wasRiding`.

```
(passengers changed → SetPassengers; item frames' maps: later)
interpolationTracker.updateTracking(position)        // NO_OP for items and falling blocks
if entity.syncPosition: tickCount = updateInterval.nextInterval(tickCount); entity.syncPosition = false
                                                     // periodic(20): (t / 20) * 20 + 20, so this tick is an update tick
if entity.needsSync || updateInterval.test(tickCount) /* tickCount % 20 == 0 */ || entityData.isDirty():
    yRot = packDegrees(yRot); xRot = packDegrees(xRot)
    rot = |yRot - lastSentYRot| >= 1 || |xRot - lastSentXRot| >= 1
    (passenger, new-minecart branches: later)
    teleportDelay++
    path = PositionPath.of(position)                 // NO_OP tracker: linear
    pos  = positionCodec.delta(position).lengthSqr() >= 7.62939453125E-6 || tickCount % 60 == 0
    packet = createMovePacket(path, yRot, xRot, pos, rot)
    if entity.needsSync || trackDelta (true) || fall flying:
        v = deltaMovement; d = v.distanceToSqr(lastSentMovement)
        if d > 1e-7 || (d > 0 && v.lengthSqr() == 0):
            lastSentMovement = v; send SetEntityMotion(id, v)       // before the move packet
    if packet != null:
        send packet; sendDirtyEntityData()
        if packet has a position: positionCodec.setBase(position)
        if packet has a rotation: lastSent rotations = these
    else: sendDirtyEntityData()
    wasRiding = false
    head: if |packDegrees(yHeadRot) - lastSentYHeadRot| >= 1 → RotateHead
    entity.needsSync = false
tickCount++
if entity.syncVelocity: entity.syncVelocity = false; send SetEntityMotion(entity) to trackers and the entity itself
```

`createMovePacket(path, y, x, pos, rot)`:
- if `entity.getRequiresPrecisePosition()` (a falling block whose last move collided)
  or `teleportDelay > 400` or `wasRiding` or `wasOnGround != entity.onGround()`:
  `wasOnGround = onGround; teleportDelay = 0`; return `EntityPositionSync.of(entity,
  path)` (the repo's `SyncEntityPosition`, absolute).
- if `pos`: `delta = positionCodec.tryEncode(path)` (each axis `Math.round(v * 4096) -
  Math.round(base * 4096)` as a long; null if any is outside `[-32768, 32767]`); if
  null, or `isFullPrecisionEncodingRequired(path.endPosition, delta)`: the sync packet;
  else `MoveEntity.PosRot(id, delta, y, x, onGround)` if `rot`, else `MoveEntity.Pos(
  id, delta, onGround)`.
- else if `rot`: `MoveEntity.Rot`; else null.

`isFullPrecisionEncodingRequired(end, delta)` — **items only**:
- if `entity.verticalCollision` and (`delta.hasDeltaX() &&
  encodingPrecisionLoss(end.x) != 0` or `delta.hasDeltaZ() && encodingPrecisionLoss(
  end.z) != 0`) → true;
- if `entity.horizontalCollision` and (`delta.hasDeltaX() && encodingPrecisionLoss(
  end.z) != 0` or `delta.hasDeltaZ() && encodingPrecisionLoss(end.x) != 0`) → true (x
  and z crossed, as the bytecode has it);
- else false. `encodingPrecisionLoss(v) = Math.round(v * 4096) / 4096.0 - v`;
  `hasDeltaX` = the encoded x delta is not 0.

So the region must report, per entity per tick, beside position, motion, rotations and
on-ground: **`horizontal_collision`, `vertical_collision`** (items), **`requires_precise
_position`** (falling blocks), **`needs_sync`** (items: touching a fluid, or the
motion changed by more than 0.1 in the tick), **`sync_position`** (set by a bounce),
**`sync_velocity`** (set by `markHurt`), and data changes (the stack, the on-fire flag).
The move packets carry `VecDelta` (`ClientboundMoveEntityPacket$Pos`: varint id,
varint `packProperties(onGround, delta.stepCount())`, then `VecDelta.write`: three
shorts for a linear delta): to write and compare with the official server.
`SetEntityMotion` (103): varint id, the motion as `LpVec3` (`Vec3.LP_STREAM_CODEC`).
`AddTransientBlock` (37): `BlockPos` (long), state id (varint).

Spawn (`sendPairingData`): `AddEntity` (the item: type, uuid, position base, last-sent
motion as `LpVec3`, rotations; a falling block: velocity zero, data =
`Block.getId(state)`), then `SetEntityData` of the non-default values (the item: index
8 `ITEM_STACK`; the falling block: index 8 `BLOCK_POS` start position). The falling
block's landing: `BlockUpdate`, then `AddTransientBlock`, then the removal.

**Client and server**: both run the item's and the falling block's physics
(`MoveSimulationType.SERVER_AND_CLIENT`); the client corrects to what the server sends
every 20 ticks, on a change of on-ground, every 60 ticks for position and on
`needsSync` (so an item in water or a falling item bouncing is sent every tick when it
changes). The client never pushes items out of blocks (`noPhysics = false`), never
merges, despawns or picks up; it counts `age` for the spin.

---------------------------------------------------------------------------------------

## 10. Not needed for items and falling blocks

Step-up (`maxUpStep` 0); `maybeBackOffFromEdge`; `limitPistonMovement` (only for
pistons moving entities, a separate part of N4: `PistonMovingBlockEntity` →
`entity.move(MoverType.PISTON, …)`); movement emission, step sounds and vibrations
(`MovementEmission.NONE`); `minorHorizontalCollision`; swimming, eyes in fluids
(`getHeightForCamera`), splash effects (cosmetic); interpolation trackers
(`InterpolationHandler.NO_OP`); passengers and vehicles; portals; freezing; the honey
block's effects for living entities; experience orbs; entity colliders (no boats or
shulkers yet); `blockData` of falling block entities; the anvil's damage and its
`AnvilBlock.damage` draw; game events (`HIT_GROUND`, `BOUNCE`, `ENTITY_DAMAGE`);
statistics and advancements of pick-up; the item's bob offset (drawn, never used on the
server); weather (`isInRain`).

---------------------------------------------------------------------------------------

## 11. What the repo needs beyond what it has

**From the jar by `tools/datagen/java/Extract.java`:**
1. **Collision shapes in the game's voxel form**: for each distinct collision shape
   (and the entity-inside shapes of item 4), the three coordinate lists
   (`VoxelShape.getCoords(axis)`) and the filled cells (`VoxelShape.shape`, a protected
   `DiscreteVoxelShape`, by reflection: sizes and `isFull(x, y, z)`), and whether it is
   the `Shapes.block()` instance (`shape == Shapes.block()`), which `BlockCollisions`
   tests by identity. Keep the boxes for the code that uses them.
2. **Per block** (`Block` getters): `getFriction()`, `getSpeedFactor()`,
   `getJumpFactor()`, `getBounceRestitution()`, `getFallDistanceReduction()`, as float
   bits; and `getMaxHorizontalOffset()` with the block's offset type, for the offset
   shapes.
3. **Per state**: `hasLargeCollisionShape()` (or derive it from item 1's bounds);
   `isCollisionShapeFullBlock` exists.
4. **Entity-inside shapes**: `getEntityInsideCollisionShape(level, pos, null)` for the
   five classes that override it (end portal, tripwire, lava cauldron, powder snow,
   water cauldrons); the rest are `Shapes.block()`.
5. **Entity types** (the plan's table, now needed): width, height, eye height,
   `clientTrackingRange`, `updateInterval`, `trackDeltas`, `fireImmune` (items:
   0.25 × 0.25, eye 0.2125, 6, 20, true, false; falling blocks: 0.98 × 0.98, 10, 20,
   true, false).
6. **Fluid ids** exist (`FLUIDS`); the tag membership of fluids is in `tags.rs`.

**From the generated data (no extraction):**
7. **Item components** in `generated/reports/minecraft/components/item/<name>.json`
   (1658 files): `minecraft:max_stack_size` (needed now: merging, pick-up, the
   creative throw's finding `bin/clustine/tests/items.rs:727`), `minecraft:
   damage_resistant` (fire immunity of netherite items), `minecraft:max_damage`
   (damageable items for `isStackable` and `isDamaged`).
8. **Block loot tables** (`generated/data/minecraft/loot_table/blocks/*.json`) and the
   two predicate files (`generated/data/minecraft/predicate/tool/*.json`), read into
   a compact table at datagen, with section 8.2's conditions and modifiers.
9. Tags used (all in `tags.rs` already): blocks `fire`, `fall_damage_resetting`,
   `suppresses_bounce`, `fences`, `walls`, `blocks_fluid_flow`, `anvil`; fluids
   `water`, `lava`; damage type `is_fire`; entity type `powder_snow_walkable_mobs`.

**Code:**
10. `clustine-blocks`: `fluid::flow` (section 4); the entity-context collision shapes
    of powder snow and scaffolding; the position offset of offset shapes.
11. `clustine-sim`: the voxel collider (`Shapes.collide`, `VoxelShape.collide`,
    `BlockCollisions`, `findSupportingBlock`, `joinIsNotEmpty` with a box), `Entity.move`
    and the per-entity transient state of 3.5, the inside-block walk
    (`forEachBlockIntersectedBetween`) and the step collector, `EntityFluidInteraction`,
    `ItemEntity.tick`, `FallingBlockEntity.tick` replacing `Region::fall`, the loot
    evaluator, `popResource` in `destroy_block` (honouring its drop flag) and in the
    fluids' washing away and leaf decay, pick-up in the players' phase after the
    entities, `Inventory.add`, a block traversal `clip` for the two fast-moving cases
    (from `placement/bucket.rs`), and the entity streams of section 8.5.
12. `EntityData`: `Item { health, … }` and `FallingBlock { state, time, drop_item,
    cancel_drop, hurt_entities, fall_damage_per_distance, fall_damage_max }`; the
    entity's `fall_distance` and `remaining_fire_ticks`.
13. The edge: `ServerEntity`'s bookkeeping per entity (section 9), `MoveEntity.Pos` /
    `PosRot` with `VecDelta`, `SetEntityMotion`, `TakeItemEntity`, `AddTransientBlock`,
    and the per-tick flags in the region's reports. To compare with the official server
    (layouts and when packets come), as the protocol rule requires.

**Decisions for the plan** (from section 0): the motion of a resting item (finding 5,
proposal 3.5); same-tick ticking of entities spawned before the entity phase (finding
6); the streams for drops (8.5); whether a survival partial pick-up should tell the
client the new count (finding 8; the game does not, **Inferred**).
