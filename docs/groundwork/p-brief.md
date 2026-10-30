# Porting brief: the Nether and the End (26.3, protocol 777)

Read from `server-26.3.jar` with `javap -p -c -constants` (Mojang's names), one file per class. Most of it comes from `/home/claude/reference/d-javap/`, and a file named without a directory is in that folder. Other dumps are named (`n7 BucketItem.txt`, `n9 Level.txt`). Data comes from the datagen output in `/home/claude/clustine/target/datagen/26.3/generated/` (dimension types, `world_preset/normal.json`, noise settings, density functions, placed and configured features, structures, tags) and from Clustine's generated tables (`crates/clustine-data/src/generated/spawns.rs`, which holds the biomes' spawners, since 26.3's biome JSON no longer does). Line ranges are those of the `javap` output.

Every rule below is the bytecode's or the data's unless it is marked:
- **Inferred**: reasoning, or memory of a class that was not dumped.
- **Unverified (Pumpkin)** or **Unverified (SteelMC)**: read only in those Rust servers.
- **Client**: what the client does.

B's conventions hold: `double` is `f64`, `float` is `f32`, and there is no fused multiply-add. Java's `/` and `%` on `int` truncate towards zero; they are not floor operations.

---------------------------------------------------------------------------------------

## 0. What the step should know first (findings)

1. **26.3 has no `ultrawarm`, `natural`, `piglin_safe`, `bed_works`, `respawn_anchor_works`, `has_raids`, `effects` or `fixed_time` field.** `DimensionType` (DimensionType.txt 2–59) is a record of:
   - `hasFixedTime`, `hasSkyLight`, `hasCeiling`, `hasEnderDragonFight`;
   - `coordinateScale`, `minY`, `height`, `logicalHeight`;
   - `infiniburn`, `ambientLight`, `monsterSettings`;
   - `skybox`, `cardinalLightType`;
   - `attributes` (an `EnvironmentAttributeMap`), `timelines`, `defaultClock`.

   What the old fields did is now done by **environment attributes** under `gameplay/…`, set in the dimension type's `attributes` and some of them overridable by biomes. Section 1 maps each old field to its successor.
2. **Clustine already makes the Nether's and the End's ground.** The biome source, fill, surface and carvers were judged by the game's own classes (roadmap, T3 to T6). The End's islands are `EndIslands` in `crates/clustine-worldgen-data/src/density.rs:827`. The fortress and the End city are assembled into pieces (`crates/clustine-structures/src/fortress.rs`, `end_city.rs`). What is missing:
   - the 12 feature kinds of batch F7 (`crates/clustine-features/src/dispatch.rs`);
   - the block placement of the fortress, bastion, End city and Nether fossil;
   - every Nether and End mob, the dragon included;
   - **serving more than one dimension at all**. The edge's `Login` carries `dimension_names: [overworld]` (`services/edge/src/fanout.rs:6934`); the generator is fixed to the overworld type (`services/worldgen/src/lib.rs:47`); there is no Respawn packet in `clustine-protocol`.

   **The largest new piece is architectural.** Each level needs its own chunks, regions, store and time, and a player or an entity must be able to move between levels.
3. **Vanilla's portal travel is synchronous and reaches far into the destination level.** In the tick it happens, `PortalForcer.findClosestPortalPosition` calls `PoiManager.ensureLoadedAndValid` over a square of radius 16 (into the Nether) or 128 (out of it), which loads chunks (PortalForcer.txt 39–81). Making a portal reads `MOTION_BLOCKING` heightmaps over a spiral of radius 16 and writes up to 2 × 3 × 4 + 30 blocks (83–489). For Clustine this is a request to whoever holds the destination, or to the store. The player then arrives a tick or more later than in vanilla. That is a question for the plan (section 10).
4. **Timing:**
   - A survival player crosses on the 81st tick in a Nether portal (`players_nether_portal_default_delay` 80); a player whose abilities are `invulnerable` (creative, spectator) crosses on the first tick (`players_nether_portal_creative_delay` 0). Sources: NetherPortalBlock.txt 162–186; n7d GameRules.txt 639–650; PortalProcessor.txt 26–56.
   - **Every other entity crosses on its first tick inside.** So a player riding a horse or a boat crosses at once: the vehicle uses the portal, not the rider.
   - After a crossing the cooldown is 10 ticks for a player and 300 for anything else, or 10 for a vehicle whose first passenger is a player (Entity.txt 7512–7525; n9 Player.txt 680–683).
5. **The client is not waited for on portal travel.** `restartClientLoadTimerAfterRespawn` (60 ticks) runs only on joining and after a death or credits respawn (ServerGamePacketListenerImpl.txt 158, 5786, 5814). A portal changes dimension with `Respawn(keep = 3)` and a position packet. Until the client accepts the teleport, its movement is ignored and the player is invulnerable to everything but ender-pearl damage (ServerPlayer.txt ~3015–3040).
6. **The dragon fight is saved data** (`ender_dragon_fight`, EnderDragonFight.txt 1823–1844) of the End level, created in `ServerLevel`'s constructor when `has_ender_dragon_fight` is set (ServerLevel.txt 296–318). It ticks only while the level is not empty-paused (`emptyTime < 300`, ServerLevel.txt ~595–620). Its 20 gateway indices are shuffled once with the world seed.
7. **The End's island function is single precision and no longer contains the central island.** `EndIslandFunction.getHeightValue` starts at `-100.0f` (EndIslandFunction.txt 13–130). The central island is `distance_to_point` in the data (`density_function/end/islands.json`). Clustine already matches the game here.
8. **Bytecode quirks a port must keep:**
   - `EnderDragonFight.isArenaLoaded` loops z from `8 + cz` to `8 + cz`: one row only (862–935).
   - The holding pattern keeps the dragon on the outer ring whenever the fight exists, even with no crystals alive (`aliveCrystals() >= 0`, DragonHoldingPatternPhase.txt `findNewTarget` 279–325).
   - `PortalProcessor` compares the portal time *before* it adds 1 (26–56).

---------------------------------------------------------------------------------------

## 1. Dimensions as data

### 1.1 The three types (DimensionTypes.txt 23–301; datagen `dimension_type/*.json`)

| Field | Overworld | The Nether | The End |
|---|---|---|---|
| `has_fixed_time` | false | true | true |
| `has_skylight` | true | false | true |
| `has_ceiling` | false | true | false |
| `has_ender_dragon_fight` | false | false | true |
| `coordinate_scale` | 1.0 | 8.0 | 1.0 |
| `min_y` / `height` / `logical_height` | −64 / 384 / 384 | 0 / 256 / 128 | 0 / 256 / 256 |
| `infiniburn` | `#infiniburn_overworld` (netherrack, magma block) | `#infiniburn_nether` (= overworld's) | `#infiniburn_end` (overworld's + bedrock) |
| `ambient_light` | 0.0 | 0.1 | 0.25 |
| `monster_spawn_light_level` | uniform 0–7 | constant 7 | constant 15 |
| `monster_spawn_block_light_limit` | 0 | 15 | 0 |
| `skybox` / `cardinal_light` | overworld / default | none / nether | end / default |
| `default_clock` | `overworld` | none | `the_end` |
| `timelines` | `#in_overworld` | `#in_nether` | `#in_end` |

`overworld_caves` is the overworld's type with `has_ceiling` true (270–301).

The gameplay attributes the three types set (lines 23–64, 128–184, 218–256). Defaults are from n7 EnvironmentAttributes.txt 827–1100.

| Attribute (default) | Overworld | Nether | End |
|---|---|---|---|
| `bed_rule` (sleep when dark, may set spawn) | when dark / always | `destroy_on_use` | `destroy_on_use` |
| `straw_bed_rule` | `destroy_on_leave` | `destroy_on_use` | `destroy_on_use` |
| `respawn_anchor_works` (false) | false | **true** | false |
| `nether_portal_spawns_piglin` (false) | **true** | – | – |
| `water_evaporates` (false) | – | **true** | – |
| `fast_lava` (false; not positional) | – | **true** | – |
| `piglins_zombify` (true) | – | **false** | – |
| `can_start_raid` (true) | – | **false** | – |
| `snow_golem_melts` (false) | – | **true** | – |
| `sky_light_level` | (timeline) | 4.0 | – |
| `increased_fire_burnout` (false) | set by some overworld biomes (jungle, swamps, peaks, mushroom fields) | – | – |

The Nether also sets fog distances 10 and 96, a sky light factor of 0 and dripping-lava dripstone particles. The End sets `background_music` to `music.end`. These are client visuals.

### 1.2 What each field (and each former field) changes

- **ultrawarm (gone)** became three attributes:
  - `water_evaporates`. Emptying a water bucket plays `fire_extinguish` (volume 0.5, pitch `2.6 + (r1 − r2) × 0.8`) and sends 8 `large_smoke` particles instead of placing water (n7 BucketItem.txt ~185–320). Ice broken without Silk Touch, or melted by light above 11 minus its dampening, is removed and leaves no water (n8 IceBlock.txt 20–100).
  - `fast_lava` (section 5.2).
  - The dripstone particle.
  - **Inferred:** dispensers, wet sponges and dripstone filling cauldrons also read `water_evaporates`. They were not dumped. Clustine names the dispenser case in `crates/clustine-sim/src/region/dispensers/behaviours.rs:29`.
- **natural (gone)** was split up:
  - Zombified piglins spawning from portal blocks became `nether_portal_spawns_piglin` (NetherPortalBlock.txt 36–101).
  - Sleeping and spawn-setting became `bed_rule`.
  - Clocks and compasses (**Client**): a clock reads the dimension's `default_clock`, and the Nether has none. A compass points to the spawn's `GlobalPos` only in that spawn's dimension. Section 5.6 has the details.
- **piglin_safe (gone):** `piglins_zombify`, read at the mob's position by `AbstractPiglin.isConverting` (AbstractPiglin.txt 167–189) and `Hoglin.isConverting` (Hoglin.txt 473–498).
- **bed_works (gone):** `bed_rule`. With `destroy_on_use`, using a bed removes both halves and explodes with power 5.0, fire on, `bad_respawn_point` damage and `BLOCK` interaction (n7 BedBlock.txt 32–75). With `can_set_spawn: never` a bed never sets a spawn there (ServerPlayer.txt 2286–2432, the bed branch).
- **respawn_anchor_works:** now an attribute (RespawnAnchorBlock.txt 271–281, section 5.4).
- **has_raids (gone):** `can_start_raid` (n7k Raids.txt ~193).
- **has_skylight:**
  - whether sections carry sky light (n7 ChunkMap, n10 SerializableChunkData);
  - daylight detectors (n8 DaylightDetectorBlock);
  - phantoms (n9 PhantomSpawner);
  - `Level.canHaveWeather` (with `!has_ceiling`, n7 Level.txt ~1560).
- **has_ceiling:** weather; the top found when mobs are placed with a new chunk (n7 NaturalSpawner.txt `getTopNonCollidingPos` ~1140); striped maps in the Nether (n10 MapItem).
- **fixed_time → has_fixed_time + timelines + default_clock.** `Level.isBrightOutside` is `!hasFixedTime && skyDarken < 4` (n9 Level.txt). So endermen never flee daylight in the Nether or the End (Enderman.txt 482–528). `skyDarken` is `15 − sky_light_level` (n9 Level.txt ~1185–1195).
- **coordinate_scale:** `getTeleportationScale(from, to) = from.scale / to.scale` (DimensionType.txt 170–181). It is used by Nether portals only.
- **min_y, height:** the number of sections. **logical_height:** `ServerLevel.getLogicalHeight` (ServerLevel.txt ~2369), which caps portal making at `min(maxY, minY + logicalHeight − 1)` (PortalForcer.txt 83–114). **Inferred:** chorus-fruit teleports too.
- **ambient_light:** `LevelReader` (n7). **Inferred:** the pathfinding and brightness weights of light-sensitive mobs; otherwise **Client**.
- **monster_spawn_light_level, monster_spawn_block_light_limit:** `Monster.isDarkEnoughToSpawn` (n7 Monster.txt, as N9 built it).
  - The Nether rejects nothing by block light (limit 15) and compares raw brightness, which is only block light there, against 7.
  - The End rejects any block light above 0 and accepts every sky light (test 15).
- **effects → skybox + cardinal_light:** client only, except `hasEndFlashes` (skybox `END`, DimensionType.txt 207–216).
- **infiniburn:** `FireBlock.tick` reads the dimension's set at the block below (FireBlock.txt 247–300, section 5.5).
- **has_ender_dragon_fight:** creates and ticks `EnderDragonFight` (section 6).

### 1.3 The level stems (`world_preset/normal.json`)

| Stem | Type | Generator |
|---|---|---|
| `minecraft:overworld` | `overworld` | noise, `multi_noise` preset `overworld`, settings `overworld` |
| `minecraft:the_nether` | `the_nether` | noise, `multi_noise` preset `nether`, settings `nether` |
| `minecraft:the_end` | `the_end` | noise, biome source `the_end`, settings `end` |

Noise settings:
- `nether`: default block netherrack, default fluid lava, sea level 32, `legacy_random_source` true, noise min 0 height 128, mobs at generation on.
- `end`: end stone, air, sea level 0, legacy random, 0 / 128, `disable_mob_generation` true.

The respawn dimension is the world spawn's `RespawnData.dimension()` if that level exists, else the overworld (MinecraftServer.txt 3660–3680). `ServerLevel.END_SPAWN_POINT` is (100, 50, 0) (ServerLevel.txt 4846–4855).

---------------------------------------------------------------------------------------

## 2. Travel

### 2.1 The portal clock (every portal kind)

- **Entering.** A portal block's `entityInside` calls `entity.setAsInsidePortal(portal, pos)` if `entity.canUsePortal(false)` (NetherPortalBlock.txt 150–160; EndPortalBlock.txt 34–64; EndGatewayBlock.txt 171–203).
- `canUsePortal(false)` is `!isPassenger() && isAlive()` (Entity.txt 9279–9293), and also `!isSleeping()` for a living entity (n7 LivingEntity.txt 10349–10360). It is false for the ender dragon (EnderDragon.txt 2954–2958) and the wither (WitherBoss.txt 1596–1599).
- **`setAsInsidePortal`** (Entity.txt 7388–7427):
  - If the entity is on cooldown (`portalCooldown > 0`), the cooldown is set again to its full value. Standing in the arrival portal keeps the entity on cooldown.
  - Otherwise, if there is no `PortalProcessor` or it belongs to another portal kind, a new one is made with this entry position and `insidePortalThisTick = true`.
  - Otherwise, if it was not marked this tick, the entry position is updated and it is marked.
- **`handlePortal`**, called from `baseTick` (Entity.txt 994; 7428–7486):
  1. `processPortalCooldown` takes 1 off the cooldown if it is positive. A player skips this while `isChangingDimension` (ServerPlayer.txt 4881–4887).
  2. If there is a processor, `processPortalTeleportation(level, entity, canUsePortal(false))` (PortalProcessor.txt 26–56):
     - If marked inside this tick: clear the mark; if allowed, compare the time *before* incrementing, `portalTime++ >= portal.getPortalTransitionTime(level, entity)`.
     - If not marked: `decayTick` sets `portalTime = max(0, portalTime − 4)` (76–87).
  3. On true: `setPortalCooldown()` (to `getDimensionChangingDelay()`), take `getPortalDestination`, and if it is not null call `teleportToPortalDestination`.
  4. On false: if the time has fallen to 0 (`hasExpired`), drop the processor.
- **Transition time.**
  - Nether portal: for a `Player`, `max(0, gamerule)` where the gamerule is `players_nether_portal_creative_delay` (0) if `abilities.invulnerable`, else `players_nether_portal_default_delay` (80); 0 for everything else (NetherPortalBlock.txt 162–186).
  - End portal and gateway: the interface default, 0 (Portal.txt).
  - **Inferred:** projectiles count too; `ThrowableProjectile` overrides `canUsePortal` (n7k).
- **Cooldown** (`getDimensionChangingDelay`, Entity.txt 7512–7525): 300, or the first passenger's own delay if that passenger is a `ServerPlayer`. A player's is 10 (n9 Player.txt 680–683).
- **Local transition:** the Nether portal returns `CONFUSION` (NetherPortalBlock.txt 510–513). That is the client's portal overlay and wobble; the server sends nothing for it.
- **`teleportToPortalDestination`** (7487–7511) teleports only if `oldLevel.isAllowedToEnterPortal(newLevel)` and, between different levels, `canTeleport(old, new)`.
  - `isAllowedToEnterPortal` is the gamerule `allow_entering_nether_using_portals` when the destination is the Nether, else true (ServerLevel.txt 4413–4429).
  - `canTeleport` is false from the End to the overworld when any passenger is a `ServerPlayer` that has not `seenCredits` (Entity.txt 9294–9318).

### 2.2 The Nether portal's destination (NetherPortalBlock.txt 188–508)

1. **Destination level:** the overworld if the source is the Nether, else the Nether. `isNether` = the destination is the Nether.
2. **Target point:** with `s = getTeleportationScale(sourceType, destType)`, the target is `destBorder.clampToBounds(x × s, y, z × s)` as a `BlockPos`. From the overworld that is ⅛, from the Nether ×8; y is unscaled.
3. **Searching for a portal** (`PortalForcer.findClosestPortalPosition`, PortalForcer.txt 39–81):
   - Radius 16 if `isNether`, else 128.
   - `poiManager.ensureLoadedAndValid(level, target, r)`, then `getInSquare(type == nether_portal, target, r, ANY)`.
   - Keep those within the destination's world border whose block still has `horizontal_axis`.
   - Take the minimum by `distSqr(target)`, then by an int key. **Inferred:** the key is `getY` (a method reference not visible in the dump).
4. **If one is found:** its rectangle is `BlockUtil.getLargestRectangleAround(pos, its axis, 21, Y, 21, same state)`. The post-transition is portal sound, then `placePortalTicket(foundPos)` (radius-3 `PORTAL` ticket, Entity.txt 9208–9227).
5. **If none is found:**
   - A spectator gets `null`: no portal is made and there is no teleport.
   - Otherwise `createPortal(target, the source block's axis or X)`. If that returns empty, the game logs "Unable to create a portal, likely target out of worldborder" and the result is `null`.
   - The post-transition is portal sound, then a portal ticket at the entity's new position.
6. **Placing the traveller** (`getDimensionTransitionFromExit` 332–380, `createDimensionTransition` 382–508, `PortalShape.getRelativePosition` 504–603):
   - In the source portal's rectangle (21 × 21 search over the same state), the relative position is:
     - along the axis: `clamp(inverseLerp(pos.axis − (min.axis + width/2), 0, axis1Size − width), 0, 1)`, or 0.5 if there is no room;
     - up: `clamp(inverseLerp(y − min.y, 0, axis2Size − height), 0, 1)`, or 0 if there is no room;
     - across: `pos.other − (min.other + 0.5)`.
     - If the source block has no axis, (0.5, 0, 0) with axis X.
   - In the destination rectangle (min corner, sizes a1 × a2):
     - along = `width/2 + (a1 − width) × rel.x`;
     - up = `(a2 − height) × rel.y`;
     - across = `0.5 + rel.z`;
     - placed on x/z according to the destination's axis.
   - `findCollisionFreePosition` (605–672) applies only to entities at most 4 × 4. It calls `ServerLevel.findFreePosition` with a box of the entity's width, centred on the point raised by height/2, extended 1 upwards and inflated by 1e−6. The result is lowered back.
   - The transition is `(destLevel, pos, Vec3.ZERO, yRot = 0 if the axes match else 90, xRot = 0, relatives = DELTA ∪ ROTATION)`. Velocity is kept, and yaw turns 90° when the axes differ.

### 2.3 Making a portal (`PortalForcer.createPortal`, 83–489)

Constants (3–27): radii 16 and 128; frame 4 wide and 5 high.

- **Variables.** `dir = POSITIVE` along the axis (east for X, south for Z). `top = min(level.getMaxY(), minY + logicalHeight − 1)`.
- **Spiral.** Positions are walked by `BlockPos.spiralAround(target, 16, EAST, SOUTH)` (33 × 33). For each column:
  1. Skip it unless the column and the column one step along `dir` are inside the border.
  2. Start at `y0 = min(top, MOTION_BLOCKING height)` and walk y down to `minY`. At each y where the block `canPortalReplaceBlock` (`canBeReplaced()` and no fluid, 491–508):
     - descend while the block below is also replaceable;
     - reject if `floorY + 4 > top`;
     - reject a gap of 1 or 2 (`0 < startY − floorY < 3`);
     - otherwise test `canHostFrame(pos, dir, 0)`.
  3. `canHostFrame(origin, dir, offset)` (510–571) checks, for i in −1..2 along `dir` and j in −1..3 up, at `offset` across: the row j = −1 must be `isSolid()` and the rows j ≥ 0 must be replaceable.
  4. A spot passing offset 0 has distance `distSqr(target)`. If it also passes offsets −1 and +1 it competes for the best **three-deep** spot; otherwise, while no three-deep spot exists, it competes for the best **one-deep** spot.
- **Fallback.** If neither kind was found:
  - `floor = max(minY − (−1), 70)`, `ceil = top − 9`; if `ceil < floor`, return empty.
  - The spot is `(target.x − dir.stepX, clamp(target.y, floor, ceil), target.z − dir.stepZ)`, clamped to the border.
  - With `cw = dir.getClockWise()`, for across ∈ −1..1, along ∈ 0..1 and up ∈ −1..2, set obsidian at up = −1 and air above. That makes a 2 × 3 obsidian floor with three blocks of air over it.
- **Frame.** For along ∈ −1..2 and up ∈ −1..3, set obsidian on the edges (`along ∈ {−1, 2}` or `up ∈ {−1, 3}`) with `setBlockAndUpdate`.
- **Portal blocks.** For along ∈ 0..1 and up ∈ 0..2, set `nether_portal[axis]` with flags 18 (clients, no neighbour shape updates).
- **Result.** `FoundRectangle(spot, 2, 3)`.

### 2.4 The End portal (EndPortalBlock.txt 34–190)

- **`entityInside`:** in the End, a `ServerPlayer` that has not `seenCredits` gets `showEndCredits()` instead of the portal (section 2.5). Everything else is `setAsInsidePortal`.
- **Into the End** (source not the End). The destination is the End, at `atBottomCenterOf(END_SPAWN_POINT)` = (100.5, 50, 0.5).
  - `EndPlatformFeature.createEndPlatform(endLevel, BlockPos.containing(that).below(), true)` is called with (100, 49, 0).
  - **Inferred** (the class is not dumped): for dx, dz ∈ −2..2 and dy ∈ −1..2 it puts obsidian at dy = −1 and air above. Blocks that differ are destroyed with drops first. That gives a 5 × 5 obsidian floor at y = 48 with air from 49 to 51.
  - The same platform is a placed feature of the `the_end` biome at fixed (100, 49, 0) (`placed_feature/end_platform.json`).
  - A **player** is put 1 lower, at (100.5, 49.0, 0.5), standing on the floor. **Any other entity** arrives at y = 50 and falls a block.
  - Yaw is `WEST.toYRot()` = 90, pitch 0, relatives `DELTA ∪ {X_ROT}` (pitch and velocity kept).
  - Post-transition: portal sound, then portal ticket.
- **Out of the End.** The destination is the level of the End level's `getRespawnData().dimension()`, i.e. the world spawn's level.
  - A `ServerPlayer` gets `findRespawnPositionAndUseSpawnBlock(false, DO_NOTHING)`: their bed or anchor without spending a charge, else the world spawn. There is no portal sound and no ticket.
  - Anything else gets `atBottomCenterOf(entity.adjustSpawnLocation(destLevel, spawnPos))` with the spawn's yaw and pitch, `DELTA ∪ ROTATION`, portal sound and ticket. **Inferred:** `adjustSpawnLocation` is the top of the column.

### 2.5 Leaving the End by the credits

`showEndCredits` (ServerPlayer.txt 2434–2460):
1. `unRide()`, then `level.removePlayerImmediately(player, CHANGED_DIMENSION)`.
2. If `!wonGame`: `wonGame = true`, send `GameEvent(WIN_GAME, 0.0)`, `seenCredits = true`.

**Client:** the client shows the End poem and credits, then sends `ClientCommand(PERFORM_RESPAWN)`.

`handleClientCommand` with `wonGame` (ServerGamePacketListenerImpl.txt 5741–5830):
1. `wonGame = false`.
2. `player = playerList.respawn(player, keepAllData = true, CHANGED_DIMENSION)`.
3. `resetPosition()`, `restartClientLoadTimerAfterRespawn()` (60 ticks).
4. Trigger `changed_dimension(END → OVERWORLD)`.

The respawn spot is `findRespawnPositionAndUseSpawnBlock(!keepAllData = false, …)`: the bed or anchor, no charge spent (PlayerList.txt 844–853). A player who has seen the credits uses the portal normally (2.4).

`seenCredits` and `wonGame` are fields of `ServerPlayer`. **Inferred:** `seenCredits` is saved with the player.

### 2.6 End gateways (TheEndGatewayBlockEntity.txt, EndGatewayBlock.txt)

Constants (3–17): spawn time 200, cooldown 40, attention interval 2400, a gateway 10 above the surface it is made over.

- **Ticks.** The server ticker is `portalTick` (116–163):
  - age + 1;
  - if cooling down, cooldown − 1;
  - else, if `age % 2400 == 0`, `triggerCooldown`;
  - `setChanged` whenever "spawning" (`age < 200`) or "cooling down" flips.
- **`triggerCooldown`** (230–249): cooldown = 40, then `level.blockEvent(pos, block, 1, 0)`. That is a `BlockEvent` packet with action 1 and parameter 0; the client sets its own cooldown of 40 and draws the beam (`triggerEvent` 251–265).
- **Entering** (`entityInside` 171–203): if `canUsePortal(false)`, on the server, and the gateway is not cooling down: `setAsInsidePortal`, then `triggerCooldown`. The transition time is 0, so the teleport is next tick.
- **Destination** (`getPortalDestination` 204–263, `getPortalPosition` 267–317):
  - If the gateway has no exit and is in the End, it finds or makes one:
    - `p = findOrCreateValidTeleportPos(level, pos).above(10)`;
    - place `EndGatewayFeature.knownExit(pos, false)` there (a return gateway pointing back);
    - set this gateway's exit to `p` (its `exactTeleport` kept).
  - The exit point is the exit itself if `exactTeleport`, else `findExitPosition`: `findTallestBlock(exit + (0, 2, 0), 5, false).above()`.
  - Result: `atBottomCenterOf`. A `ThrownEnderpearl` gets an absolute transition with zero velocity; anything else gets `DELTA ∪ ROTATION` and a portal ticket.
- **`findOrCreateValidTeleportPos`** (340–392):
  1. `findExitPortalXZPosTentative` (394–459): the direction is `normalize(x, 0, z)` of the gateway. Start 1024 along it. Step back 16 at a time while the chunk there is non-empty, up to 16 steps. Then step forward 16 at a time while it is empty, up to 16 steps. "Empty" means `getHighestFilledSectionIndex() == −1`.
  2. `findValidSpawnInChunk` (569–662): among positions from (minX, 30, minZ) to (maxX, top of the highest section, maxZ), the end stone with two non-full-collision blocks above it that is nearest by `distToCenterSqr(0, 0, 0)`.
  3. If none: place the `end_island` feature with `RandomSource.create(pos.asLong())` at `(x + 0.5, 75, z + 0.5)` (lambdas 743–766).
  4. Finally `findTallestBlock(level, pos, 16, true)` (474–551): in the square of radius 16, for each column from `maxY` down to the best y so far, the first full-collision block (bedrock allowed). With `allowBedrock` false the centre column is skipped and bedrock is refused.
- **Gateway blocks** (`EndGatewayFeature.place`, EndGatewayFeature.txt 45–192), a 3 × 5 × 3 box around the centre:
  - the centre is `end_gateway`, with its exit set if the feature has one;
  - the rest of the centre layer is air;
  - at y ± 1 the plus shape (same x or same z) is bedrock and the corners are air;
  - at y ± 2 the centre is bedrock and the rest is air.
- **The return gateways of `end_highlands`:** `rarity_filter 700`, `in_square`, `MOTION_BLOCKING` heightmap, offset y uniform 3–9, configured `exact: true, exit: [100, 50, 0]` (datagen).

### 2.7 Who travels, and what they ride

- **Entity.teleport** (Entity.txt 8733–8779): if the transition is not `asPassenger`, `stopRiding()`. A `ServerPlayer` calls `removeVehicle()` (ServerPlayer.txt 2490–2494).
- **Within one level:** passengers are teleported first with `calculatePassengerTransition` (offset from the vehicle kept, rotation deltas kept; 8960–9039), then the vehicle is moved and the teleport is sent (8780–8827, 9040–9120).
- **Across levels** (`teleportCrossDimension`, 8828–8929):
  1. Note the passengers and `ejectPassengers()`.
  2. Teleport each passenger first, as a passenger.
  3. `EntityType.create(newLevel, DIMENSION_TRAVEL)`; `restoreFrom(old)`; `old.removeAfterChangingDimensions()`; set the position; `newLevel.addDuringTeleport(new)`.
  4. Each arrived passenger `startRiding(new, true, false)`.
  5. `newLevel.resetEmptyTime()`, the post-transition, then `teleportSpectators`: spectators whose camera was this entity follow it and are released from it (8930–8959).
  - **Inferred:** the new entity has the same UUID and a new network id.
- **Leashes:** `removeAfterChangingDimensions` (9228–9265) removes the old entity as `CHANGED_DIMENSION`, calls `Leashable.removeLeash()` on a leashed one, and untracks a waypoint transmitter. **A leashed animal loses its leash.** **Inferred:** the lead is dropped.
- **Mounts:** a player in a boat or on a horse cannot use a portal themselves (they are a passenger). The vehicle does, at once, and carries the player.
- **Pearls:** **Inferred.** An ender pearl crossing takes its owner's pearl with it, and pearls in other levels are saved with the player (`loadAndSpawnEnderPearls`, ServerPlayer.txt 726–760).

---------------------------------------------------------------------------------------

## 3. What the client is sent when a player changes dimension

### 3.1 By a portal (`ServerPlayer.teleport(TeleportTransition)`, ServerPlayer.txt 2462–2635)

Before anything:
- If the transition has `missingRespawnBlock`: `GameEvent(NO_RESPAWN_BLOCK_AVAILABLE = 0, 0.0)`.
- If not `asPassenger`: `removeVehicle()`.
- Same level: `connection.teleport(pos, relatives)` (a `PlayerPosition` packet), `resetPosition()`, the post-transition; done.

Another level, in this order:
1. `isChangingDimension = true`.
2. **`Respawn(CommonPlayerSpawnInfo, dataToKeep = 3)`**. 3 is `KEEP_ALL_DATA` = `KEEP_ATTRIBUTE_MODIFIERS (1) | KEEP_ENTITY_DATA (2)` (ClientboundRespawnPacket.txt 3–18).
   `CommonPlayerSpawnInfo` (5376–5410), in order:
   - dimension type (a registry holder);
   - dimension key;
   - `BiomeManager.obfuscateSeed(seed)`;
   - game mode;
   - previous game mode (optional);
   - `isDebug`, `isFlat`;
   - last death location (optional);
   - **`portalCooldown`**;
   - sea level.
   **Inferred** (ClientboundRespawnPacket.txt 76–82): the wire form is the `CommonPlayerSpawnInfo` codec followed by a byte.
3. **`ChangeDifficulty(difficulty, locked)`** of the new level's data.
4. **`PlayerList.sendPlayerPermissionLevel`**. **Inferred:** an `EntityEvent` 24–28 for the operator level.
5. `oldLevel.removePlayerImmediately(player, CHANGED_DIMENSION)`, then `unsetRemoved()`. The old level's `ChunkMap` applies an empty tracking view, so `PlayerChunkSender.dropChunk` runs for every sent chunk (n7 ChunkMap.txt 1792–1800, 2364–2420). **Inferred:** these are `ForgetLevelChunk` packets sent *after* the Respawn, which the client ignores, and the old level's tracked entities are removed for this player.
6. From the overworld to the Nether: `enteredNetherPosition = position`.
7. `setServerLevel(new)`.
8. **`connection.teleport(pos, relatives)`**: `awaitingTeleport` + 1, wrapping at `Integer.MAX_VALUE`; `awaitingTeleportTime = tickCount`; the position is set; `awaitingPositionFromClient` = the position; send **`PlayerPosition(id, pos, delta, yRot, xRot, relatives)`** (ServerGamePacketListenerImpl.txt 3812–3870).
9. `resetPosition()`.
10. `newLevel.addDuringTeleport(player)` → `addPlayer` (ServerLevel.txt 2029–2043). From here the new level's `ChunkMap` sends chunks in batches and the tracked entities, as at a join (**Inferred**, as Clustine already does).
11. `triggerDimensionChangeTriggers(old)`: `changed_dimension`, and `nether_travel` from the Nether to the overworld (2657–2691); `stopUsingItem()`.
12. **`PlayerAbilities`**.
13. **`sendLevelInfo(player, newLevel)`** (PlayerList.txt 1643–1715):
    - `InitializeBorder`;
    - the clock manager's full `SetTime`;
    - `SetDefaultSpawnPosition(level.getRespawnData())`;
    - if raining: `GameEvent(START_RAINING, 0)`, `RAIN_LEVEL_CHANGE(rainLevel)`, `THUNDER_LEVEL_CHANGE(thunderLevel)`;
    - **`GameEvent(LEVEL_CHUNKS_LOAD_START = 13, 0)`**, which starts the client's loading screen;
    - the tick rate manager's join update.
14. **`sendAllPlayerInfo`** (1716–1750): `inventoryMenu.sendAllDataToRemote()` (**Inferred:** `SetContainerContent` of the inventory), `resetSentInfo()`, `SetHeldSlot(selected)`.
15. **`sendActivePlayerEffects`**: one `UpdateMobEffect(id, effect, false)` for each active effect (1111–1143).
16. `sendPostEffects()`: `PostEffects`.
17. The post-transition (portal sound: `LevelEvent(1032, ZERO, 0, false)` to this player only, TeleportTransition.txt 88–107; the portal ticket).
18. `lastSentExp = −1`, `lastSentHealth = −1`, `lastSentFood = −1`. **Experience, health and food are therefore resent by the next player tick**, not in this method.
19. `teleportSpectators`.

### 3.2 By respawning (`PlayerList.respawn`, PlayerList.txt 844–1109)

This is used after death (`keepAllData` false) and after the credits (true).

1. Find the spot.
2. Remove the old player from the list and level.
3. Make a **new** `ServerPlayer` with the same connection; `restoreFrom(old, keepAllData)`; same entity id; main arm, tags and post effects copied; `snapTo`.
4. Send, in order:
   - `NO_RESPAWN_BLOCK_AVAILABLE` if the bed or anchor was gone;
   - **`Respawn(info, keepAllData ? 1 : 0)`** (attribute modifiers only, or nothing);
   - **`PlayerPosition`** (`teleport(x, y, z, yRot, xRot)`, absolute);
   - `SetDefaultSpawnPosition` of the respawn level;
   - `ChangeDifficulty`;
   - **`SetExperience(progress, total, level)`**;
   - effects;
   - post effects;
   - `sendLevelInfo`;
   - the permission level.
5. `addRespawnedPlayer`, then `initInventoryMenu()`, `setHealth(getHealth())`.
6. After a death at an anchor, a `Sound(respawn_anchor_deplete)` at the anchor.

### 3.3 What the client must answer

- **`AcceptTeleportation(id)`** with the id of the position packet.
  - Until it arrives, `awaitingPositionFromClient` is set and movement packets are not taken.
  - **Inferred:** the position is resent after 20 ticks (`updateAwaitingTeleport` 3698–3790).
  - On a matching id with valid values (1130–1217): `absSnapTo(awaited)`, record the last good position, **`hasChangedDimension()`** (clears `isChangingDimension`, so the cooldown counts down again and damage applies), clear the awaited position, then `handlePlayerPositionChange` with the client's values.
- **`PlayerLoaded`** is needed only after joining, death or credits (`hasClientLoaded` = `!waitingForRespawn && clientLoadedTimeoutTimer <= 0`, 6770–6820, timeout 60). Until then `PlayerAction`s are ignored and the player is invulnerable. On portal travel the server does not wait for it.
  - **Client:** the client sends it anyway when its loading screen ends; the server treats it as a no-op.
- While `isChangingDimension`, `isInvulnerableTo` returns true for all but `ender_pearl` damage (ServerPlayer.txt ~3015–3040). **Inferred:** the movement checks are relaxed (ServerGamePacketListenerImpl.txt 3333, 3584).

**Registries.** The `Login` must list all three dimension names. The dimension-type holder in `Respawn` is the id of the type in the `dimension_type` registry as sent in configuration (Clustine sends the core pack's entries without data).

---------------------------------------------------------------------------------------

## 4. Making portals

### 4.1 The Nether portal frame (`PortalShape`, PortalShape.txt)

- **Size.** The frame is obsidian (`#nether_portal_frame` = `obsidian`). Width 2–21 and height 3–21 count the inside (3–9, `isValid` 429–450).
- **`findAnyShape(level, pos, axis)`** (93–157). Here `rightDir` is WEST for axis X and SOUTH for Z, so the left is east or north.
  - **`calculateBottomLeft`** (159–201): descend while the block below `isEmpty` (air, `#fire` or `nether_portal`), no further than `max(minY, y − 21)`. Then go left by `getDistanceUntilEdgeAboveFrame(opposite) − 1`; if that is negative there is no shape.
  - **`getDistanceUntilEdgeAboveFrame(pos, dir)`** (221–266): for i in 0..21, the block at i must be empty with frame below it; the first non-empty block must be frame and gives i; otherwise 0.
  - **Width:** that distance to the right, accepted only if 2–21 (203–219).
  - **`getDistanceUntilTop`** (331–409): for each row h < 21, the left post at −1 and the right post at `width` must be frame (else h); each inside block must be empty (else h); `nether_portal` blocks are counted.
  - **Height:** accepted if 3–21 and the whole top row 0..width−1 at height h is frame (`hasTopFrame` 301–329).
  - **Corners are never examined.**
- **`isComplete`** (486–502) means valid and the count of portal blocks = width × height.
- **Lighting.** `BaseFireBlock.onPlace` (448–484): when any fire (fire or soul fire) is placed by any means where the block was not fire before, and the level is the overworld or the Nether (`inPortalDimension` 485–499), `PortalShape.findEmptyPortalShape(level, pos, X)` is tried (valid and holding no portal blocks yet). If that fails, Z is tried (53–91).
  - On success `createPortalBlocks` sets `nether_portal[axis]` over the rectangle with flags 18 (452–484, 683–691) and the fire is replaced.
  - If neither fits, the usual "remove if it cannot survive" applies.
  - **No portal can be lit in the End.**
- **Flint and steel** (FlintAndSteelItem.txt 10–130) places fire on the clicked face if `BaseFireBlock.canBePlacedAt(level, pos, horizontalDirection)` (523–550): the spot is air, and the fire would survive there **or** `isPortal` (551–610). `isPortal` holds in the overworld or Nether when some neighbour is frame and an empty shape exists on the axis `dir.getCounterClockWise().getAxis()`; for a vertical direction a random horizontal axis is drawn from the level random.
  - **Inferred:** the fire charge uses the same check (`FireChargeItem` not dumped).
  - The fire placed is soul fire on `#soul_fire_base_blocks` (soul sand, soul soil), else fire (`getState` 32–53).

### 4.2 When a Nether portal breaks (`NetherPortalBlock.updateShape`, 103–148)

On a neighbour change from direction `d`, the block stays if any of these holds:
- `d`'s axis is horizontal and differs from the portal's axis (front and back are ignored);
- the neighbour is a portal block;
- `findAnyShape(level, pos, axis).isComplete()`.

Otherwise it becomes air. A frame block mined, a portal block removed or the shape changed therefore drops the whole portal through the neighbour cascade.

**Zombified piglins from portals** (36–101): on a random tick, if the level spawns monsters, difficulty is not peaceful, `nether_portal_spawns_piglin` holds at the position and `random.nextInt(2000) < difficulty.getId()`:
- go down through the portal blocks to the block under them;
- if a player is close enough and that block `isValidSpawn` for a zombified piglin, spawn one above it with reason `STRUCTURE`;
- set its portal cooldown, and its vehicle's.

### 4.3 The End portal frame and eyes (EnderEyeItem.txt, EndPortalFrameBlock.txt)

- **Filling a frame** (`useOn` 10–129). On an `end_portal_frame` with `eye = false`, on the server:
  1. Set `eye = true` with flags 2 and `pushEntitiesUp`.
  2. Comparator update, shrink the stack by 1, `levelEvent(1503)`.
  3. Find `getOrCreatePortalShape()` (131–222): the aisle `?vvv?`, `>???<` × 3, `?^^^?`, a 5 × 5 ring of frames with eyes, each facing inwards (south, west, north, east).
  4. On a match: from `frontTopLeft + (−3, 0, −3)`, `destroyBlock(…, true)` and set `end_portal` with flags 2 over the inner 3 × 3, then `globalLevelEvent(1038)` at its centre.
- **Throwing** (`use` 135–…). Unless aiming at a frame:
  1. Start using the item.
  2. On the server, `findNearestMapStructure(#eye_of_ender_located, playerPos, 100, false)`.
  3. If found: an `EyeOfEnder` at (x, y(0.5), z) with the item, `signalTo(atLowerCornerOf(found))`, the game event, `addFreshEntity`, the `used_ender_eye` trigger, the `ender_eye_launch` sound with pitch `0.4 / (lerp(r, 0.33, 0.5))`; consume 1, award the use statistic.
- **Flight.** `signalTo` (EyeOfEnder.txt 116–165):
  - if the horizontal distance is above 12, the target is `pos + (dx/d × 12, 8, dz/d × 12)`, else the point;
  - `life = 0`;
  - it survives with probability 4 in 5 (`nextInt(5) > 0`).
- **Each tick** (167–265): the motion is steered towards the target; after 80 ticks it plays `ender_eye_death`, is discarded, and drops its item or plays `levelEvent(2003)`.

---------------------------------------------------------------------------------------

## 5. Rules that differ in the Nether and the End

### 5.1 Water
Water evaporates where `water_evaporates` holds (1.2).

### 5.2 Lava (`fast_lava`)
- Entities in lava are pushed by `0.007` instead of `0.0023333333333333335` (Entity.txt ~4330–4345; N4 brief).
- **Inferred**, `LavaFluid` not dumped, agreeing with **Unverified (SteelMC)** `steel-core/src/fluid/fluids/lava.rs` 61–70:
  - tick delay 10 instead of 30;
  - drop-off 1 instead of 2, so lava flows 7 blocks instead of 3;
  - slope-find distance 4 instead of 2.
- Pumpkin gives 5 and 3 for the last of these and is not followed.
- Source conversion is the gamerule `lava_source_conversion`.

### 5.3 Beds
A bed explodes when slept in (1.2). Straw beds use `straw_bed_rule`.

### 5.4 Respawn anchors (RespawnAnchorBlock.txt)

- **Charging.** Glowstone charges an anchor below 4 charges: `charge + 1` with `setBlockAndUpdate`, a game event and the `respawn_anchor_charge` sound, and 1 glowstone consumed (31–65, 282–332). This works in every dimension. With glowstone in the off hand and the anchor chargeable, the main hand passes.
- **Using it empty-handed with charge > 0** (66–152):
  - Where `respawn_anchor_works` is false (the overworld, the End) it **explodes**: the block is removed, then an explosion of power 5.0 with fire on, `bad_respawn_point` damage and `BLOCK` interaction (215–270).
    - If water that would flow is beside it horizontally (a source, or a flowing level of at least 2 with no water below), or water is above, the anchor's own position takes water's resistance (`RespawnAnchorBlock$1`).
  - In the Nether a `ServerPlayer` whose respawn config is not already this spot gets `setRespawnPosition(RespawnConfig(of(dim, pos, 0, 0), forced = false), true)` and the `respawn_anchor_set_spawn` sound.
- **Respawning at it** (ServerPlayer.txt 2286–2432): needs `charge > 0` or `forced`, `canSetSpawn` at that level, and `findStandUpPosition`. One charge is spent when not forced and the caller asks (death respawn yes, End portal and credits no).

### 5.5 Fire
- Fire on a block of the dimension's `infiniburn` never burns out, and rain does not put it out (FireBlock.txt 247–300). That is netherrack and magma everywhere, plus **bedrock in the End**: the fire on the spikes' bedrock, and under End crystals.
- `increased_fire_burnout` (−50 to the chance) is biome-set and absent in these dimensions (274–304).
- **Soul fire** survives only on `#soul_fire_base_blocks` (SoulFireBlock.txt 26–41). **Inferred:** it does 2 damage to fire's 1 (the `fireDamage` passed to `BaseFireBlock`, Blocks not dumped). Piglins avoid it (`#piglin_repellents`).

### 5.6 Compasses, lodestones and clocks
- `LodestoneTracker.tick` (LodestoneTracker.txt 23–68), for a tracked compass, acts only in its target's dimension: if the target is outside the world's bounds or there is no `lodestone` point of interest there, the target is cleared (tracked stays true).
- **Client:** the needle spins outside the target's dimension. A plain compass points to the world spawn's `GlobalPos` only in that dimension. A clock reads the level's default clock: it spins in the Nether (none) and shows the End's clock in the End.

### 5.7 Piglins and hoglins
- **Zombification.** `isConverting` = not immune, has AI, and `piglins_zombify` at the position. Each `customServerAiStep` counts `timeInOverworld` up while converting and resets it otherwise. Above 300 ticks, and not in peaceful, it plays the converted sound and becomes a zombified piglin, or a zoglin for a hoglin, with nausea 200 ticks (AbstractPiglin.txt 127–259; Hoglin.txt 165–204, 363–380, 473–498, 599). `TimeInOverworld` and `IsImmuneToZombification` are saved.
- **Bartering** (PiglinAi.txt):
  - the currency is a gold ingot (`BARTERING_ITEM`);
  - admiring lasts 119 ticks; a dropped item is walked to from up to 9 blocks for up to 200 ticks;
  - angered for 600 ticks when gold is taken or mined in range 16;
  - the drops are the `gameplay/piglin_bartering` loot table (`getBarterResponseItems` 817–842), thrown towards the player or a random spot (747–816);
  - `isWearingSafeArmor` checks for gold armour.
  - This is a brain-driven mob of N7j's kind.

### 5.8 Zombified piglins and endermen
- **Zombified piglins** (ZombifiedPiglin.txt):
  - attributes: speed 0.23, attack 5.0, reinforcement chance 0;
  - in anger they alert others of their type within the follow range's box inflated by 10 vertically (229–255);
  - anger lasts a random 20–39 seconds; alerts every 4–6 seconds;
  - spawn rule: not peaceful and the block below is not `nether_wart_block` (313–…);
  - they spawn with a golden sword.
  - Nothing in them depends on the dimension except the portal spawns of 4.2.
- **Endermen:** only the daylight teleport depends on the dimension (1.2).

### 5.9 Striders, ghasts, blazes, magma cubes, wither skeletons
- **Striders** (Strider.txt, 1245 lines):
  - spawn placement `IN_LAVA`; the spawn rule climbs through lava and needs air above (56–90);
  - warm when standing in `#strider_warm_blocks` (lava), cold and slowed outside (`tick` 707–…, 140 and 60);
  - steered with a warped fungus on a stick; ridden speed `speed × (suffocating ? 0.35 : 0.55)`;
  - they walk on lava (the liquid collision shape is 8 high).
- **Ghasts:**
  - attributes: health 10, follow range 100, flying speed 0.06 (Ghast.txt 193–233);
  - spawn rule: not peaceful, `nextInt(20) == 0` and the mob rules (235–…);
  - immune to their own fireballs but take 1000 from one a player reflected (114–192).
  - The fireball goal (`GhastShootFireballGoal.tick` 47–…): with a target within 64 and in line of sight, `chargeTime` + 1; at 10 `levelEvent(1015)`; at 20 `levelEvent(1016)` and a `LargeFireball` aimed from 4 blocks ahead of the eyes with `explosionPower` (default 1), then `chargeTime = −40`.
  - **Inferred** for the fireball (not dumped): it explodes with fire, and `mob_griefing` decides the blocks.
- **Blazes** (Blaze.txt, Blaze$BlazeAttackGoal.txt 69–…):
  - attack 6, speed 0.23, follow 48;
  - melee within 2 (4 squared);
  - ranged: after 60 ticks of charging, three `SmallFireball`s 6 ticks apart with `levelEvent(1018)` and a spread of `sqrt(sqrt(d)) × 0.5`, then 100 ticks of rest;
  - hurt by water.
- **Magma cubes:** a cube mob of the slime's kind (Clustine's `cube_mob.rs`), speed 0.2, armour 3 × size, never "on fire" visually, a jump of `0.42 + 0.1 × size`.
- **Wither skeletons:** a stone sword; their hit gives wither for 200 ticks (WitherSkeleton.txt 105–…); their arrows are flaming (`getArrow`, fire for 100); immune to wither; they target piglins as well.

### 5.10 Spawning
- **The Nether's biomes** (`spawns.rs`, all with `creature_probability` 0.1):
  - nether wastes: strider 60 (1–2, creature); ghast 50 (4); zombified piglin 100 (4); magma cube 2 (4); enderman 1 (4); piglin 15 (4);
  - soul sand valley: strider 60; skeleton 20 (5); ghast 50 (4); enderman 1 (4); costs of 0.15 / 0.7 for skeleton, ghast, enderman and strider;
  - crimson forest: strider 60; zombified piglin 1 (2–4); hoglin 9 (3–4); piglin 5 (3–4);
  - warped forest: strider 60; enderman 1 (4); a cost of 0.12 / 1.0 for endermen;
  - basalt deltas: strider 60; ghast 40 (1); magma cube 100 (2–5).
- **The fortress** (`structure/fortress.json`): in a fortress *piece*, for monsters on `nether_bricks` (n7 NaturalSpawner.txt 692–730): blaze 10 (count uniform 2–3), zombified piglin 5 (4), wither skeleton 8 (5), skeleton 2 (5), magma cube 3 (4). The count is now a value provider.
- **The End:** every End biome has enderman 10 (4); `the_void` has none. The End has `disable_mob_generation`.
- **Placements** (n9 SpawnPlacements-v.txt 1055–1385): all on the ground except the strider (`IN_LAVA`) and the shulker (`NO_RESTRICTIONS`).
- **Rules:**
  - piglin and hoglin: not above `nether_wart_block`;
  - ghast: as above;
  - zombified piglin: as above;
  - magma cube (MagmaCube.txt 59–69): not peaceful.
  - **Inferred:** the blaze uses `checkAnyLightMonsterSpawnRules`.

---------------------------------------------------------------------------------------

## 6. The End's dragon fight

### 6.1 `EnderDragonFight` (saved data)

- **What is saved** (codec 1694–1760), with defaults from `createDefault` (67–83):
  - `needs_state_scanning` (true);
  - `dragon_killed` (false);
  - `previously_killed` (false);
  - `respawn_stage` (optional);
  - `respawn_time` (0);
  - `dragon_uuid` (optional);
  - `exit_portal_location` (optional);
  - `gateways` (a list of ints);
  - `respawn_crystals` (entity references).
- **`init(level, seed, origin = ZERO)`** (307–376):
  - a boss bar named `entity.minecraft.ender_dragon`, pink, `PROGRESS` overlay, boss music and world fog;
  - `validPlayer` = alive and within 192 of (0, 128, 0);
  - if `gateways` is empty: the list 0..19 shuffled with `RandomSource.createThreadLocalInstance(seed)`, then `setDirty`.
- **`tick`** (385–521):
  1. The bar is visible iff `!dragonKilled`.
  2. Every 20 ticks, `updatePlayers`: add every valid player to the bar and remove the others (937–988).
  3. With no player on the bar, remove the `DRAGON` ticket (radius 9 at chunk 0, 0) and stop.
  4. Otherwise add the ticket and stop unless `isArenaLoaded`. That requires chunks x −8..8 at **z = +8 only** to be full and block-ticking (the quirk of section 0), and entities loaded in the 3 × 3 chunks around the origin (862–935).
  5. If `needs_state_scanning`: `scanState` (523–600), then clear the flag.
  6. If a respawn is in progress: the living respawn crystals; if none, `abortRespawnSequence`; else `respawnStage.tick(level, this, crystals, respawnTime++)`.
  7. If not killed: if there is no dragon UUID or 1200 ticks pass unseen, `findOrCreateDragon` (602–630). Every 100 ticks, `updateCrystalCount`: the crystals inside each spike's top box (990–1029).
- **The first fight.** `scanState`:
  - no `TheEndPortalBlockEntity` in chunks −8..8 → not previously killed;
  - if `findExitPortal` finds nothing, `spawnExitPortal(false)`;
  - with no dragon alive, `dragonKilled` would be true, but a never-killed fight sets it back to false.
  - Then `findOrCreateDragon` → `createNewDragon` (1253–1328): load the chunk at (0, 128, 0); an `ender_dragon` (reason `EVENT`) with this fight and origin, phase `HOLDING_PATTERN`, at (0, 128, 0) with yaw `nextFloat() × 360` from the level random.
- **The exit portal.**
  - `findExitPortal` (727–860) matches the 7 × 5 × 7 bedrock pattern (89–252) at any End portal block entity within ±8 chunks, else down the column of the podium from its `MOTION_BLOCKING` height. It remembers `match.getBlock(3, 3, 3)`.
  - `spawnExitPortal(active)` (1164–1251):
    - if no location yet: `MOTION_BLOCKING_NO_LEAVES` height at (0, 0, 0), then `below()`; descend while it is bedrock and y > 63; clamp to at least `minY + 1`;
    - place `end_podium_active` or `end_podium_inactive` with `RandomSource.create()`;
    - on success, `waitForLightBeforeSending` over radius `positiveCeilDiv(4, 16)`.
- **`setDragonKilled`** (1031–1076), for the fight's own dragon:
  1. Bar progress 0, hidden.
  2. `spawnExitPortal(true)`.
  3. `spawnNewGateway()`.
  4. **On the first kill only,** a dragon egg at the `MOTION_BLOCKING` height of the podium.
  5. `previouslyKilled = dragonKilled = true`.
- **Gateways** (1087–1162): take the last index i of the list. The position is `(floor(96 cos(2(−π + 0.15707963267948966 i))), 75, floor(96 sin(…)))`. Send `levelEvent(3000)` there, then place `end_gateway_delayed` (no exit, not exact) with `RandomSource.create()`.
- **The bar.** `updateDragon` (1330–1357) sets the progress to health / max and resets the unseen count; a named dragon names the bar.

### 6.2 Respawning the dragon

- **Trigger.** `EndCrystalItem.useOn`: on obsidian or bedrock with air above and no entity in the 1 × 2 × 1 box, an `EndCrystal` at (x + 0.5, y, z + 0.5) without a base, then `tryRespawn()`.
- **`tryRespawn`** (1430–1507): if killed and no respawn is under way, find (or make, active) the exit portal. For each horizontal direction there must be a crystal in the block box `exit.above(1).relative(dir, 3)`; then `respawnDragon(crystals)`.
- **`respawnDragon`** (1509–1595):
  1. While `findExitPortal` matches, turn every bedrock or end-portal block of the match into end stone.
  2. Stage `START`, time 0.
  3. `spawnExitPortal(false)`.
  4. Remember the crystals.
- **The stages** (`DragonRespawnStage$1`–`$5`):
  - **START:** every crystal beams at (0, 128, 0); go to PREPARING_TO_SUMMON_PILLARS.
  - **PREPARING_TO_SUMMON_PILLARS:** `levelEvent(3001)` at (0, 128, 0) on ticks 0, 50, 51, 52 and 95–99; at 100 go to SUMMONING_PILLARS.
  - **SUMMONING_PILLARS:** with `k = time / 40` over the spikes in order:
    - on `time % 40 == 0` the crystals beam at `(cx, h + 1, cz)` of spike k;
    - on `time % 40 == 39`: remove blocks in the ±10 box around `(cx, h, cz)`; explode power 5.0 with `BLOCK` interaction at `(cx + 0.5, h, cz + 0.5)`; rebuild the spike with `EndSpikeFeature(List.of(spike), true, Optional.of((0, 128, 0)))` at `(cx, 45, cz)`.
    - When `k` reaches the spike count on a `% 40 == 0` tick, go to SUMMONING_DRAGON.
  - **SUMMONING_DRAGON:**
    - time 0: beam at (0, 128, 0);
    - times 1–4 and 80–99: `levelEvent(3001)`;
    - at 100: `setRespawnStage(END)`, `resetSpikeCrystals()` (crystals mortal, no beam), and each respawn crystal loses its beam, explodes with power 6.0 and `NONE` interaction, and is discarded.
  - **END:** `setRespawnStage(END)` (632–682) clears the stage, sets `dragonKilled = false`, creates the dragon, and gives every bar player `summoned_entity`.
- **Abort** (1404–1422), when a respawn crystal is destroyed or none remain: clear the stage, `resetSpikeCrystals`, `spawnExitPortal(true)`.

### 6.3 Crystals and spikes

- **`EndCrystal`** (EndCrystal.txt):
  - each tick: time + 1, block effects, `handlePortal`; in a level with a dragon fight, fire (`BaseFireBlock.getState`) where its block is air (61–101);
  - hurt by anything but the dragon (158–214): removed as `KILLED`; unless the damage is an explosion, an explosion of power 6.0 without fire and with `BLOCK` interaction, attributed to the attacker; then `fight.onCrystalDestroyed`;
  - `beam_target` and `ShowBottom` are saved.
- **`fight.onCrystalDestroyed`** (1365–1402): a respawn crystal aborts the respawn. Otherwise the crystal count is updated and the dragon's `onCrystalDestroyed` runs (EnderDragon.txt 2854–2906):
  - the attacker, if a player, else the nearest player within `CRYSTAL_DESTROY_TARGETING` of the crystal;
  - if it was the dragon's `nearestCrystal`, 10 explosion damage to the head;
  - then the phase's own handler (the holding pattern strafes that player).
- **Spikes** (EndSpikeFeature$SpikeCacheLoader.txt 9–…):
  - for seed key `RandomSource.createThreadLocalInstance(worldSeed).nextLong() & 0xFFFF` (EndSpikeFeature.txt 32–49, cached 5 minutes), the values 0..9 are shuffled into `v[i]`;
  - spike i stands at `(floor(42 cos(2(−π + 0.3141592653589793 i))), floor(42 sin(…)))`;
  - radius `2 + v/3`, height `76 + 3v`, caged iff `v ∈ {1, 2}`;
  - the top box is `(cx − r, MIN_Y, cz − r)` to `(cx + r, MAX_Y, cz + r)`.
- **`placeSpike`** (89–413), for the spike whose centre chunk is the feature's:
  - over `(cx − r, minY, cz − r)` to `(cx + r, h + 10, cz + r)`: obsidian where `distToLowCornerSqr(cx, y, cz) <= r² + 1` and `y < h`, else air above y 65;
  - if caged: iron bars at dx, dz ∈ −2..2 and dy ∈ 0..3 from h where `|dx| = 2 || |dz| = 2 || dy = 3`, connected along the cage;
  - a crystal at `(cx + 0.5, h + 1, cz + 0.5)`, random yaw from the feature's random, with the feature's beam and invulnerability;
  - bedrock under it and fire (`FireBlock.getState`) in its block.
  - In world generation the feature is `end_spike` with no spikes, so `getSpikesForLevel` applies, mortal and beamless.

### 6.4 The dragon (`EnderDragon`, 3032 lines)

- **Basics:**
  - health 200, camera distance 16 (255–265);
  - eight parts: head, neck (3 × 3), body (5 × 3), three tail parts, two wings. **The client numbers them `dragonId + 1 … dragonId + 8`** (`recreateFromPacket` 2959–2986), so a server must keep those ids free;
  - no effects, cannot ride, cannot use portals;
  - `DragonPhase` and `DragonDeathTime` are saved (2684–2733).
- **Healing** (`checkCrystals` 1349–1424): while `nearestCrystal` is alive, every 10 ticks health + 1 up to the maximum. One tick in ten (`nextInt(10) == 0`) the nearest `EndCrystal` within the box inflated by 32 is chosen.
- **Damage to it** (`hurt(part, …)` 1717–1803): nothing while dying. The phase's `onHurt` first; a sitting phase turns arrows and wind charges into 0 damage and sets them on fire for 1 second (AbstractDragonSittingPhase.txt).
  - Non-head, non-neck parts take `a/4 + min(a, 1)`.
  - Below 0.01 nothing happens.
  - Only player sources or `#always_hurts_ender_dragons` hurt it.
  - While sitting, more than 25 % of max health taken makes it take off.
  - The killing blow outside a sitting phase sets health 1 and phase `DYING` (`handleKillingBlow` 1825–1840).
- **Its own attacks** (aiStep 341–1303, knockBack 1425–1533, hurt 1534–1570):
  - the wings knock back living entities that are not creative or spectators away from the body; unless sitting, they also do 5 damage to those not hurt in the last 2 ticks;
  - the head and neck do 10 to living entities;
  - `checkWalls` (1578–1716) removes non-air, non-`#dragon_transparent` blocks in its parts' boxes when `mob_griefing` holds and they are not `#dragon_immune`, with `levelEvent(2008)` at a random block of the box.
- **Phases** (`EnderDragonPhase` ids 0–10, in order):
  - **HoldingPattern (0):** follows nodes of a 24-node graph (12 outer, 8 middle, 4 inner; `findClosestNode` 2089–2414). On reaching its target (distance² < 100 or > 22500, or a collision) it picks anew:
    - with probability `1 / (crystals + 3)`, LandingApproach;
    - else for the nearest player to the podium (line of sight ignored), `d = distToCenterSqr / 512` (64 without a player); StrafePlayer if `nextInt(|d| + 2) == 0` or `nextInt(crystals + 2) == 0`;
    - else a new path: the closest node; one in eight times the direction flips and the target moves by 6; ±1; kept on the outer ring mod 12 while the fight exists.
    - Waypoints get a random height of up to +20.
  - **StrafePlayer (1):** flies at the player and shoots one `DragonFireball` (`levelEvent(1017)`), then HoldingPattern.
  - **LandingApproach (2):** paths to the node nearest to a point 40 from the podium, opposite the nearest player, at y 105; then Landing.
  - **Landing (3):** speed 1.5; within 1 block of the podium top it resets the flame count and goes to SittingScanning.
  - **SittingScanning (6):** looks for a player within 20 (150 for line of sight); after 25 ticks with one, SittingAttacking; after 100 ticks without, Takeoff or ChargingPlayer.
  - **SittingAttacking (7):** a roar; after 40 ticks, SittingFlaming.
  - **SittingFlaming (5):** at tick 10, an `AreaEffectCloud` of dragon's breath with instant damage, radius 5, duration 200, at the head's direction 2.5 out, placed on the first non-air block below. At 200 ticks: Takeoff if 4 flames have been done, else SittingScanning.
  - **Takeoff (4):** paths away from the podium (40, 105, 40 scale); once 10 above the podium, HoldingPattern.
  - **ChargingPlayer (8):** speed 3, at a remembered point; HoldingPattern after 10 ticks near it.
  - **Dying (9):** flies to the podium.
  - **Hover (10):** stays still.
- **Death** (`tickDeath` 1880–2088):
  - `dragonDeathTime + 1`;
  - explosion particles at 180–200;
  - experience pool `xp` = **12000 on the first kill, 500 after**;
  - after tick 150, every 5 ticks, `ExperienceOrb.award(floor(xp × 0.08))` while `mob_drops` holds;
  - at tick 1, `globalLevelEvent(1028)` (the death roar);
  - it rises 0.1 a tick;
  - at 200: `award(floor(xp × 0.2))`, `setDragonKilled`, removed.
  - In total that is 9,600 + 2,400 or 400 + 100.

---------------------------------------------------------------------------------------

## 7. Generation specific to these dimensions

### 7.1 The End islands
`EndIslandFunction`, the data's `end_outer_islands`, is already in Clustine and judged.

- **The noise** (`compileSampler` 131–150): `createEndIslandRandom()` (**Inferred:** a `LegacyRandomSource` seeded with the world seed), `consumeCount(17292)`, then `SimplexNoise(random)`. `SimplexNoise.get(x, z)` returns a `float` in 26.3.
- **`getHeightValue(noise, x, z)`** (13–130), all in float:

  ```
  i = x / 2;  j = z / 2;  k = x % 2;  l = z % 2;   // Java int division and remainder
  f = -100.0f
  for dx in -12..=12, dz in -12..=12:
      X = (long)(i + dx);  Z = (long)(j + dz)
      if X*X + Z*Z > 4096 && noise.get((double)X, (double)Z) < -0.9f:
          g = (abs((float)X) * 3439.0f + abs((float)Z) * 147.0f) % 13.0f + 9.0f
          h = (float)(k - dx*2);  m = (float)(l - dz*2)
          n = clamp(100.0f - sqrt(h*h + m*m) * g, -100.0f, 80.0f)
          f = max(f, n)
  return f
  ```
- **The sampler** (EndIslandFunction$Sampler.txt 61–96): `(getHeightValue(noise, blockX / 8, blockZ / 8) − 8.0f) / 128.0f`. It ignores y, and a volume fills each column at once (14–60). The range is −0.84375..0.5625.
- **The island in the middle** (`density_function/end/islands.json`, cached):

  ```
  max( slice(y = 0, (clamp(100 − euclidean distance to (0, 0, 0), −100, 80) − 8) × 0.0078125),
       end_outer_islands )
  ```
  `end/sloped_cheese` adds `end/base_3d_noise` (old blended noise, factors 80 and 160, scales 0.25, smear 4).

### 7.2 The End biome source (TheEndBiomeSource.txt 102–186)

- Section coordinates `sx`, `sz` of the quart's block.
- If `sx² + sz² <= 4096` (long arithmetic): `the_end`.
- Otherwise `h` = the router's erosion (the islands function) sampled at block `((2sx + 1) × 8, blockY, (2sz + 1) × 8)` and widened to double:
  - `h > 0.25`: highlands;
  - `h >= −0.0625`: midlands;
  - `h < −0.21875`: small islands;
  - otherwise barrens.

### 7.3 End features (placed features, datagen)

- **`the_end` biome:** `end_spike` (6.3), then `end_platform` at fixed (100, 49, 0).
- **`end_highlands`:** `end_gateway_return` (2.6) and `chorus_plant` (count 0–4, `in_square`, `MOTION_BLOCKING`).
- **`small_end_islands`:** `end_island_decorated`: rarity 1 in 14, count 1 (weight 3) or 2 (weight 1), `in_square`, height uniform 55–70.
- **`EndIslandFeature.place`** (EndIslandFeature.txt 16–…):

  ```
  r = nextInt(3) + 4.0f;  y = 0
  while r > 0.5f:
      for x in floor(-r)..=ceil(r), z in floor(-r)..=ceil(r):
          if x*x + z*z <= (r + 1)*(r + 1): set end_stone at origin + (x, y, z)
      r -= nextInt(2) + 0.5f;  y -= 1
  ```
- **Not dumped:** `end_podium` (active and inactive) and `chorus_plant`.

### 7.4 The Nether beyond pure data

- The `nether` preset of the multi-noise biome source and the noise settings are already in Clustine.
- **F7's kinds** for the Nether: `huge_fungus`, `netherrack_replace_blobs`, `delta_feature`, `random_neighbor_spread`, plus `bonus_chest` and `void_start_platform` (not the Nether's) (`dispatch.rs` 36–58).
- **Structures:**
  - `nether_complexes`: random spread, spacing 27, separation 4, salt 30084232; fortress weight 2, bastion weight 3;
  - `nether_fossils`;
  - `ruined_portals`, whose Nether setup has `placement: in_nether` and `replace_with_blackstone`;
  - `end_cities`: spacing 20, separation 11, triangular, salt 10387313.
  - The fortress and End city pieces are assembled by `clustine-structures`; their blocks, the bastion's templates and the fossils still need placing (`clustine-features/src/structures` has none of them).
- The End city's shulkers and its elytra frame are placed by its pieces. **Inferred:** `EndCityPieces` is in the dump but was not read here.

---------------------------------------------------------------------------------------

## 8. Size and order of the work

The estimates are lines of Rust with tests, measured against N9 (about 70,600 lines) and N10 (30,000–45,000 lines).

| Phase | What | Lines | Seen within minutes? |
|---|---|---|---|
| D0 | **Three levels in one world.** The dimension in chunk keys, region identities, the store's layout and the coordinator's routing; the player's saved place includes the level; joining in any level (`Login` with three names, the type's registry id); per-level time, clocks and weather (none in the Nether); serving the Nether's and End's terrain through `worldgen`; an ADR | 7,000–10,000 | Prerequisite |
| D1 | **Players through Nether portals.** `PortalShape` and lighting (fire's `onPlace`, flint and steel's portal check), portal updates and breaking, `PortalProcessor` and cooldowns, the destination search over the `nether_portal` point of interest in the other level, `createPortal`, placement and velocity, the Respawn sequence of 3.1 handed between regions of different levels, the ticket, piglins from portals | 6,000–8,500 | **Yes**: the first thing a player does |
| D1b | **The Nether's F7 features and structure blocks:** fungi, deltas, blobs, glowstone, the fortress, bastion, fossils and the Nether's ruined portals | 5,000–8,000 | **Yes**: an empty Nether is noticed at once |
| D2 | **Nether rules:** water, lava speed, beds, anchors with respawning, infiniburn per level, soul fire, lodestones, raids off, snow golems melting | 2,000–3,000 | Yes, for water and beds |
| D3 | **Nether mobs and spawning,** in this order: zombified piglin, magma cube, ghast with large fireballs, wither skeleton and blaze with small fireballs and the fortress list, strider (riding), piglin (brain, bartering, zombification), hoglin and zoglin, piglin brute | 14,000–20,000 | Yes, for the first four |
| D4 | **Entities through portals:** mobs, items, projectiles, pearls, boats and mounts with riders, leashes, spectators | 2,500–4,000 | Soon |
| D5 | **The End:** eyes of ender (locating the stronghold), frames, the End portal, the platform, the End's features (spikes, podium, islands, chorus, gateways), the return portal and the credits (`WIN_GAME`, `PERFORM_RESPAWN`, respawn with keep 1), gateways' exit search, End cities' blocks and shulkers | 7,000–10,000 | Yes, once a player gets there |
| D6a | **The dragon in its first fight:** saved data, boss bar, tickets, crystals and healing, parts with consecutive ids, flight, the holding pattern, strafing, landing, the sitting phases, dragon fireballs and breath clouds, death, experience, egg, exit portal, the first gateway | 7,000–10,000 | **Yes**: the dragon is the first thing seen on arrival |
| D6b | **Later:** the respawn sequence (crystals, stages, rebuilt spikes), the arena-loading quirks, rarer phases (charging, hover), end-of-fight edge cases across restarts | 2,500–4,000 | No |
| – | Tests from the specification by someone else; comparisons with the official server (portal positions, packet order, spikes, gateways, the dragon's path draws) | 9,000–13,000 | – |
| | **All** | **about 62,000–90,000** | |

**Order.** D0 → D1 together with D1b, since a lit portal leading to a bare Nether is a seam → D2 → D3 (the first four mobs before piglins' brains) → D4 → D5 → D6a → D6b.

- The first trial with a real client after D1: light a portal in survival and in creative; check the arrival on a new or existing portal 1:8 away; return; and check the timing (81 ticks against 1).
- D6b, the arena quirks and the respawn sequence are what a player meets only after a whole fight.

---------------------------------------------------------------------------------------

## 9. Not dumped (to dump before planning)

All of these are dumped in `/home/claude/reference/d-javap/` since 2026-10-30 (P1),
with `Level`, `CollisionGetter` (where `findFreePosition` is), `Blocks`, the cauldron
classes (`CauldronInteractions`) and `TeleportRandomlyConsumeEffect` (the chorus
fruit's teleport). The steps that read them complete this brief from them first.

- **Fluids and blocks:** `LavaFluid`, `WaterFluid` (fast lava; evaporation in other places), `WetSpongeBlock`, `PointedDripstoneBlock`, the cauldron and dispenser water paths.
- **End features:** `EndPlatformFeature`, `EndPodiumFeature`, `ChorusPlantFeature` (in d-javap but not read), `ChorusFlowerBlock`, the chorus fruit's teleport.
- **Projectiles:** `AbstractHurtingProjectile`, `LargeFireball`, `SmallFireball`, `ThrownEnderpearl`; `DragonFireball` is in d-javap but was not read.
- **Utilities and the client loading:** `BlockUtil.getLargestRectangleAround`, `BlockPos.spiralAround`, `WorldBorder.clampToBounds`, `PoiManager.ensureLoadedAndValid`, `ServerLevel.findFreePosition`, `Entity.adjustSpawnLocation`, `PlayerChunkSender`, `CommonPlayerSpawnInfo`, the client's handling of `WIN_GAME` and `LEVEL_CHUNKS_LOAD_START`.
- **Fire:** `FireChargeItem`; `Blocks` (fire damage values).
- **Mobs:** `EnderDragonPart`, `PiglinBrute`/`PiglinBruteAi`, `HoglinAi` and `Zoglin` (in d-javap, not read), `ZombifiedPiglin`'s equipment chances, `EndCityPieces` (in d-javap, not read).

## 10. Open questions for the plan

1. **Where the dragon fight lives.** Recommended: a world-level record in the store, published like N10's raids, ticked by the region that holds chunk (0, 0) of the End. The alternative is pinning the whole arena (±8 chunks) to one region.
2. **The portal search across levels.** Option A: ask the store for the destination's portal points of interest and heightmaps, and teleport on a later tick. Option B: keep the player in the portal until the destination region answers. A is recommended, with the arrival delay named and measured against vanilla's same-tick arrival.
3. **Each level's regions.** Recommended: regions are per level and never span two, so a dimension change is a hand-over between regions like a long-distance teleport.
4. **Whether joins wait for `PlayerLoaded`.** Recommended: no, as vanilla does on portals.
5. **Whether the stale `ForgetLevelChunk`s sent after a Respawn are reproduced.** Recommended: not until a comparison needs them.

### Critical Files for Implementation
- /home/claude/reference/d-javap/net.minecraft.server.level.ServerPlayer.txt
- /home/claude/reference/d-javap/net.minecraft.world.level.portal.PortalForcer.txt
- /home/claude/reference/d-javap/net.minecraft.world.level.dimension.end.EnderDragonFight.txt
- /home/claude/clustine/services/edge/src/fanout.rs
- /home/claude/clustine/crates/clustine-features/src/dispatch.rs