# Respawn, beds, anchors and the credits in 26.3: facts from the dumps

Unless marked otherwise, `SP` = d-javap/net.minecraft.server.level.ServerPlayer.txt, `PL` = d-javap/…PlayerList.txt, `SGPL` = d-javap/…ServerGamePacketListenerImpl.txt and `TT` = d-javap/…portal.TeleportTransition.txt. AbstractBedBlock and BedRule come from n7d-javap, DismountHelper and BlockGetter from n5-javap, and Direction from n6-javap; d-javap has none of them. `PlayerSpawnFinder` is not dumped anywhere. It was read straight from the server jar that `cargo datagen` downloads, with a small class-file reader, so its citations are bytecode offsets.

## 1. Finding the respawn (`SP.findRespawnPositionAndUseSpawnBlock(boolean useSpawnBlock, post)`, 2122–2170)
1. The level is `RespawnConfig.getDimensionOrDefault` (the overworld if the config is null). If the level or the config is null, the result is `TeleportTransition.createDefault`: the world spawn **without** the missing flag (2124–2169; TT 117–143). Otherwise `findRespawnAndUseSpawnBlock`: when it finds a place, a transition there with velocity ZERO and that yaw and pitch; when it does not, `TeleportTransition.missingRespawnBlock` (TT 145–171). That is the same world spawn with `missingRespawnBlock = true`.
2. `findRespawnAndUseSpawnBlock(level, config, useSpawnBlock)` (SP 2286–2432) reads the stored `pos`, `yaw`, `pitch` and `forced`, and the block state at `pos`.
   - **Anchor** (block instanceof `RespawnAnchorBlock`, 2310–2357): needs `forced || CHARGE > 0`, then `RespawnAnchorBlock.canSetSpawn(level,pos)`. That is the environment attribute `RESPAWN_ANCHOR_WORKS` at that position (RespawnAnchorBlock 271–281), so there is no other dimension rule. Then `findStandUpPosition`. **A charge is spent** with `setBlockAndUpdate(pos, CHARGE-1)` only when `!forced && useSpawnBlock && found` (2330–2350). An anchor that fails these checks falls through to the bed test and then to the forced test.
   - **Bed** (instanceof `AbstractBedBlock`, 2358–2383): needs `getBedRule(level,pos).canSetSpawn(level)` (the bed's environment attribute; `CAN_SLEEP_WHEN_DARK` has canSetSpawn=ALWAYS, `DESTROY_ON_USE` and `DESTROY_ON_LEAVE` have NEVER: n7d BedRule 157–). Any bed state counts: **PART is not checked**, and FACING is read from the state at `pos`. Then `AbstractBedBlock.findStandUpPosition(PLAYER, level, pos, FACING, storedYaw)`. A bed is never used up.
   - **Forced, any other block** (2384–2432): when `forced`, if `block.isPossibleToRespawnInThis(state)` and the same holds for the block above, the result is `(x+0.5, y+0.1, z+0.5)` with the **stored yaw and pitch**. Otherwise empty.
3. **Anchor stand-up order** (RespawnAnchorBlock 442–498, static init 512–590): pass 1 with `checkDangerous = true`, then pass 2 with false. Each pass walks `RESPAWN_OFFSETS`:
   - the horizontals (0,0,-1), (-1,0,0), (0,0,1), (1,0,0), (-1,0,-1), (1,0,-1), (-1,0,1), (1,0,1);
   - the same eight one block lower (`Vec3i::below`);
   - the same eight one block higher (`Vec3i::above`);
   - then (0,1,0).

   The jar's bootstrap methods confirm below comes before above. Each candidate goes to `DismountHelper.findSafeDismountLocation`.
4. **Bed stand-up** (n7d AbstractBedBlock 432–613, 693–953). Let f = FACING and s = `f.getClockWise()`. If `s.isFacingAngle(storedYaw)`, that is `-sin(yaw)·s.x + cos(yaw)·s.z > 0` with Mth sin and cos (n6 Direction), then s = `s.getOpposite()`.

   If the block below `pos` is also a bed, it is a bunk bed: surround(dangerous on) at y, then surround at y-1, then above, then surround(dangerous off) at y, then surround at y-1, then above(dangerous off).

   Otherwise `surround ++ above` is tried with dangerous on, then again with it off. All candidates are at `pos.y`; dx and dz are added.
   - surround = s, s−f, s−2f, −2f, −s−2f, −s−f, −s, −s+f, f, s+f
   - above = (0,0), −f
5. **`DismountHelper.findSafeDismountLocation(type, level, p, checkDangerous)`** (n5 292–390):
   - if checking, a dangerous block at p means none;
   - floor = `getBlockFloorHeight(nonClimbableShape(p), nonClimbableShape(p.below))`. That is the top of p's shape, or else the top of the shape below minus 1 if that is at least 1, or else −∞ (n5 BlockGetter). Climbables and open trapdoors count as empty;
   - the floor must be finite and below 1;
   - if checking and floor ≤ 0, a dangerous block below means none;
   - the result is `upFromBottomCenterOf(p, floor)`;
   - the player's box there must have no block collisions, `p` and `p.above` must not be in `#invalid_spawn_inside`, and the box must lie within the world border.

## 2. Yaw and pitch
- **Bed and anchor:** `RespawnPosAngle.of(pos, blockPos, 0)` (SP lambdas 5789–5803): pitch **0** and yaw = `wrapDegrees(Mth.atan2(d.z, d.x) * 57.2957763671875 − 90)`, where d = normalize(`atBottomCenterOf(blockPos)` − standPos) (RespawnPosAngle 24–55). In other words the player faces the bed or anchor. `Mth.atan2` is the game's table approximation, not `Math.atan2` (n5 Mth). The stored yaw only picks the bed's side (§1.4).
- **Forced:** the stored yaw and pitch.
- **World spawn** (createDefault and missingRespawnBlock): the respawn level's `RespawnData.yaw()` and `pitch()` (TT 117–171). The first join also uses them (PrepareSpawnTask 94–96).

## 3. A missing or invalid block
- The world spawn of `MinecraftServer.findRespawnDimension()` is used, with `missingRespawnBlock = true` (TT 145–171).
- `GameEvent(NO_RESPAWN_BLOCK_AVAILABLE = 0, 0.0)` is sent in `PL.respawn` whenever the flag is set, **whatever keepAllData is**. So it goes out after a death and also after the credits (PL 949–958). It is also sent in `SP.teleport` for the End-portal exit (SP 2470–2480).
- The config is **cleared** on the respawn path: `copyRespawnPosition` is called only when the flag is not set (PL 896–900), and the new `ServerPlayer` starts with a null config. On the End-portal path (`SP.teleport`, the same player object) nothing clears it, so **it is kept**.
- A null config (never set) gives `createDefault`: the world spawn and **no** event.

## 4. Placing on the world spawn
- `findAdjustedSharedSpawnPos` gives `atBottomCenterOf(player.adjustSpawnLocation(level, spawn.pos))` (TT 173–183).
- `SP.adjustSpawnLocation` = `BlockPos.containing(PlayerSpawnFinder.findSpawn(level,pos).join())`, with the server spinning in `managedBlock` while it waits (SP 315–333).
- `findSpawn`:
  - If the **world's default game type** is ADVENTURE, the result is `fixupSpawnHeight(spawn)`: raised while the player's box collides or holds liquid (up to maxY), then lowered while it is free (down to minY), then +1.
  - Otherwise the radius r = `max(0, respawn_radius)`. The rule's default is **10** (n6 GameRules 606–618). Then `d = floor(worldBorder.getDistanceToBorder(x,z))`; if d < r then r = d, and if d ≤ 1 then r = 1.
- The candidate scheme (`<init>`, scheduleNext):
  - count = min(1024, (2r+1)²);
  - coprime = count ≤ 16 ? count−1 : 17;
  - offset = `RandomSource.createThreadLocalInstance().nextInt(count)`, a fresh unseeded random each time;
  - candidate i (from 0 to count−1) is idx = (offset + coprime·i) % count, at x = spawn.x + idx%(2r+1) − r and z = spawn.z + idx/(2r+1) − r.

  Each candidate's chunk is loaded with a `SPAWN_SEARCH` ticket of radius 0, then `getLevelRespawnPos(level,x,z)` (the T8 column rule) runs. It is accepted if non-null and `noCollisionNoLiquid`. If all of them fail, the result is `fixupSpawnHeight(spawn)`.
- The y is the column rule's (the block above the first full-top-face block), or fixupSpawnHeight's.
- **The first join uses the same `findSpawn`** (PrepareSpawnTask 234–239), so the respawn and the first join do not differ.
- **Contradicts p-brief 2.4** ("Inferred: adjustSpawnLocation is the top of the column"): for a player it is this random spread over the radius.

## 5. `PL.respawn(old, keepAllData, reason)` (844–1109)
1. `findRespawnPositionAndUseSpawnBlock(!keepAllData, DO_NOTHING)`. So **the death spends an anchor charge and the credits do not** (846–853).
2. The old player is removed from the list and the level. A new `ServerPlayer` is made in the new level, its connection is set, then `restoreFrom(old, keepAllData)`, then setId(old), main arm, `copyRespawnPosition` unless missing, tags, post effects, `snapTo(pos, yRot, xRot)`.
3. Packets in order:
   - *(from restoreFrom: `onUpdateAbilities` → **`PlayerAbilities`**, SP 4019, 4415–4431; the connection is already set at PL 71–74, so this goes **before** Respawn; not in brief 3.2)*;
   - `NO_RESPAWN_BLOCK_AVAILABLE` if missing;
   - `Respawn(info, keepAllData ? 1 : 0)` (959–965);
   - `PlayerPosition` (absolute, the player's yaw and pitch);
   - `SetDefaultSpawnPosition(level.getRespawnData())`;
   - `ChangeDifficulty`;
   - `SetExperience(progress, total, level)`;
   - `sendActivePlayerEffects`;
   - `sendPostEffects`;
   - `sendLevelInfo`;
   - `sendPlayerPermissionLevel`;
   - then `addRespawnedPlayer`, list and UUID map, `initInventoryMenu` (the inventory), `setHealth(getHealth())`.
4. Only when `!keepAllData`, the config is non-null, its level exists and the block at its pos **is still `RESPAWN_ANCHOR`**: `Sound(respawn_anchor_deplete, BLOCKS, x, y, z as the integer block corner, 1, 1, level random seed)`, to this player (1057–1107). There is no test that a charge was spent: a forced config on an anchor also plays it.
5. `handleClientCommand` PERFORM_RESPAWN (SGPL 5741–5834):
   - with `wonGame`: wonGame = false, `respawn(player, true, CHANGED_DIMENSION)`, resetPosition, restartClientLoadTimerAfterRespawn, the `changed_dimension(END → OVERWORLD)` trigger;
   - otherwise, only if health ≤ 0: `respawn(player, false, KILLED)`, the same two calls, and in hardcore SPECTATOR.
6. `SP.restoreFrom(old, keep)` (3999–4157):
   - **always:** warden tracker, chat session, game mode and the previous one (+ abilities), base attribute values, enchantment seed, ender chest, skin parts, lastSentExp/Health/Food = −1, recipe book, **seenCredits**, enteredNetherPosition, explosion impact, chunkTrackingView, debug subscriptions, shoulder entities, last death location, waypoint icon;
   - **keep = true:** permanent attribute modifiers, **health**, **foodData**, **effects** (copied with `addEffect`), inventory, XP level, total and progress, and score (`transferInventoryXpAndScore`, 4160–4182), portalProcess;
   - **keep = false:** health = max health; inventory, XP and score only if `keepInventory` or spectator; food and effects reset.
   - **The respawn config is not copied here** (see §3).

## 6. The credits
- `EndPortalBlock.entityInside` (34–64): in the END, a `ServerPlayer` with `!seenCredits` gets `showEndCredits()` instead of `setAsInsidePortal`.
- `Entity.canTeleport` (Entity 9294–9330) refuses END → OVERWORLD for a vehicle that carries a player without seenCredits.
- `showEndCredits` (SP 2434–2460):
  - `unRide()`;
  - `level.removePlayerImmediately(this, CHANGED_DIMENSION)`, so **the player is removed from the level**;
  - if `!wonGame`: wonGame = true, `GameEvent(WIN_GAME = 4, 0.0)` (the value is always 0), seenCredits = true.
- `seenCredits` is saved and loaded as `"seenCredits"` (SP 372, 469).
- While wonGame:
  - `handleMovePlayer` returns early (SGPL 2959);
  - the idle-kick is skipped (SGPL 262);
  - `ThrownEnderpearl.tick` checks it (ThrownEnderpearl 398).

  No other handler checks wonGame. The connection still calls `player.doTick()` (SGPL 270–).

## Could not determine
- Whether `restoreFrom`'s `addEffect` (keep = true) sends `UpdateMobEffect` packets before Respawn, and under which entity id (setId comes after restoreFrom). It is likely, but LivingEntity.addEffect and onEffectAdded were not traced.
- `Block.isPossibleToRespawnInThis` and `#invalid_spawn_inside` membership, and which bed attribute each dimension uses (see brief 1.2). DismountHelper, AbstractBedBlock and Mth come from the n5, n6 and n7d dumps, not d-javap, and are taken as unchanged in 26.3.
- Entity's base `adjustSpawnLocation` for non-players was not checked.
