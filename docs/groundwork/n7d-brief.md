# N7d brief: sleep, beds, the respawn point and the overworld's clock, as 26.3 has them

Written from bytecode for a port to Clustine, for a world that is only the overworld.

## 0. Sources and how to read the references

- `D:` is a file in `/home/claude/reference/n7d-javap/` (javap -c -p of the 26.3 server jar).
  `D:Player` stands for `net.minecraft.world.entity.player.Player.txt`, and so on. A
  reference `D:ServerPlayer.startSleepInBed@89` is the bytecode offset inside that method.
- `D7:`, `D5:` are the older dumps (`n7-javap`, `n5-javap`) of the same jar.
- `J:` is a class read straight from the jar that `cargo datagen` downloaded
  (`target/datagen/26.3/versions/26.3/server-26.3.jar`), for classes that no dump has
  (`KeyframeTrackSampler`, `EasingType`, `MinecraftServer`, `Direction`, `BlockGetter`, the
  bootstrap methods that say what each `invokedynamic` calls). It was read with a small
  class-file reader (Python, in memory; no Java was run); its offsets are the same bytecode
  offsets javap prints.
- `JSON:` is a file under `/home/claude/reference/n7d-data/data/minecraft/`. Biome files and
  the language file were read from the same jar (`target/datagen/26.3/generated/...` and
  `assets/minecraft/lang/en_us.json` in the jar).
- **Inferred** marks a rule that is not in the bytecode read but follows from it or from the
  game's usual behaviour; **Not read** marks something the port needs that was not read.

The flow in one paragraph: a right click on either half of a bed is handled on the head;
the bed rule (an environment attribute, in the overworld always "sleep when dark, always
set the spawn") is read; the player is checked for distance, a clear space above, sets
their respawn point, is refused if it is not dark or monsters are near, else lies down at
the head (pose `SLEEPING`, `sleeping_pos`, bed `occupied`). Each level tick checks the
level's `SleepStatus`; when enough players sleep and enough of them have slept 100 ticks,
the overworld clock jumps to the next multiple of 24000, every client is sent the clock,
everyone is woken, and rain is stopped. "Dark" is `(int)(15 - sky_light_level) >= 4`, where
`sky_light_level` is 15 times the `day` timeline's factor (plus weather): in clear weather
the clock's day time 12523 to 23477 inclusive.

---

## 1. Using a bed

### 1.1 The block, its parts and properties

- `AbstractBedBlock` (D:AbstractBedBlock) is the parent of `BedBlock` (the 16 coloured beds)
  and `StrawBedBlock`. Properties: `FACING` (horizontal), `PART` (`BedPart`: `head`/`foot`)
  and `OCCUPIED` (boolean); defaults `PART=foot`, `OCCUPIED=false` (constructor @16-41,
  `createBlockStateDefinition`).
- `FACING` points from the foot to the head: placement puts the head at
  `clickedPos.relative(FACING)` (`setPlacedBy`@11-43) and `getNeighbourDirection(part,
  facing)` is `facing` for the foot and `facing.opposite` for the head (@0-15).
- The two halves keep `OCCUPIED` equal: `updateShape`@0-76, when the neighbour in the
  bed's direction changes and is the same block with the other part, returns this state
  with the neighbour's `OCCUPIED`; if the neighbour is not the other half, it returns air
  (@77). So a change to the head's `OCCUPIED` (all the code sets it on the head) reaches the
  foot by the shape update, and breaking one half breaks the other.
- Sleep height (`getSleepHeight`@0-45): the maximum Y of the block's shape at that state.
  `BedBlock`'s shape (D:BedBlock `lambda$static$0`): a leg `box(0,0,0,3,3,3)` and
  `column(16, 3, 9)`, so the top is 9/16 = **0.5625** for both halves. `StrawBedBlock`
  (D:StrawBedBlock `getSleepHeight`@0-73) answers for the head by measuring the foot (the
  block at `head.relative(getConnectedDirection(head))`, which must be this block with
  `PART=foot`, else empty) and the foot's shape is `column(16, 0, 4)` only, so 4/16 =
  **0.25**; the head's shape adds the pillow `box(0,0,0,16,5,8)`.

### 1.2 `useWithoutItem` (D:AbstractBedBlock `useWithoutItem`)

On the server, in this order:

1. @11-70: `part = state.PART`; the other half is at
   `pos.relative(getNeighbourDirection(part, FACING))`. If that block is not this same block
   (`is(this)`), or its `PART` equals this part, return `CONSUME` and do nothing more (no
   message).
2. @74-102: if the clicked part is not the head, `pos = pos.relative(FACING)` and `state =`
   the head's state. **Everything below uses the head's position and state.**
3. @103-109: `bedRule = getBedRule(level, headPos)` = `level.environmentAttributes()
   .getValue(getBedEnvironmentAttribute(), headPos)` (@0-15), which reads the attribute at
   `Vec3.atCenterOf(headPos)` (D:EnvironmentAttributeReader). `BedBlock` uses
   `EnvironmentAttributes.BED_RULE` (`gameplay/bed_rule`), `StrawBedBlock` uses
   `STRAW_BED_RULE` (`gameplay/straw_bed_rule`).
4. @111-148: if `bedRule.destroyOnUse()`: `errorMessage.ifPresent(player::sendOverlayMessage)`
   (J: bootstrap 0 of `AbstractBedBlock` is `Player.sendOverlayMessage`), then
   `destroyOnUse(...)`. Never in the overworld (both rules have `destroy_on_use` false, 1.3).
   For the record, `BedBlock.destroyOnUse` removes both halves and explodes at the head's
   centre with power 5.0, fire on, `ExplosionInteraction.BLOCK`, damage source
   `badRespawnPointExplosion`; `StrawBedBlock.destroyOnUse` plays `STRAW_BED_BREAK_LEAVE` and
   sets the head to air.
5. @149-187: if the head's `OCCUPIED` is true: `kickVillagerOutOfBed(level, headPos)` (a
   villager whose box intersects `new AABB(headPos)` and `isSleeping()` is told
   `stopSleeping()`, and that returns true); if no villager was kicked, the player is sent
   the overlay `block.minecraft.bed.occupied`. Return `SUCCESS_SERVER` either way; the player
   does not lie down.
6. @188-212: `player.startSleepInBed(this, headState, bedRule, headPos)`; if it returns a
   problem (`Either.left`), `lambda$useWithoutItem$0`: if the problem's message is not null,
   `player.sendOverlayMessage(message)`. Return `SUCCESS_SERVER`.

`sendOverlayMessage` (D:ServerPlayer@4531) is `sendSystemMessage(component, true)`: a
`ClientboundSystemChatPacket` with `overlay = true`, i.e. **the action bar**, sent only if the
player's chat settings accept it (`acceptsSystemMessages(true)`, @4577). `sendSystemMessage(c)`
is the same with `overlay = false` (chat).

### 1.3 The bed rule

`BedRule` (D:BedRule) is a record `(canSleep: Rule, canSetSpawn: Rule, destroyOnUse: bool,
destroyOnLeave: bool, errorMessage: Optional<Component>)`; JSON fields `can_sleep`,
`can_set_spawn`, `destroy_on_use` (default false), `destroy_on_leave` (default false),
`error_message` (optional text component) (`lambda$static$0`).

`BedRule.Rule` (D:BedRule$Rule) is `always` / `when_dark` / `never`; `test(level)`
(@0-54): `ALWAYS` true, `WHEN_DARK` `level.isDarkOutside()`, `NEVER` false.
`canSleep(level)` and `canSetSpawn(level)` test the two rules. `asProblem()` is
`new BedSleepingProblem(errorMessage.orElse(null))`.

The attribute (D:EnvironmentAttributes static init @874-913): `gameplay/bed_rule`, type
`AttributeTypes.BED_RULE` (not interpolated), default `BedRule.CAN_SLEEP_WHEN_DARK`
(`when_dark`, `always`, false, false, `block.minecraft.bed.no_sleep`); `gameplay/straw_bed_rule`
default `DESTROY_ON_LEAVE` (`when_dark`, `never`, false, true, `block.minecraft.bed.no_sleep`).
Both are positional (no `notPositional()`), so a biome could override them.

Values in the overworld:

- JSON:dimension_type/overworld.json `attributes`:
  `gameplay/bed_rule = {can_set_spawn: always, can_sleep: when_dark, error_message:
  {translate: block.minecraft.bed.no_sleep}}`;
  `gameplay/straw_bed_rule = {can_set_spawn: never, can_sleep: when_dark,
  destroy_on_leave: true, error_message: {translate: block.minecraft.bed.no_sleep}}`.
- No timeline of the overworld (`#minecraft:in_overworld` = `villager_schedule`, `day`,
  `moon`, `early_game`) has a track for either, and no biome of the jar sets either (the
  biome files' attributes are only `natural_mob_spawns`, `sky_color`, `background_music`,
  `water_fog_color`, `fog_color`, `increased_fire_burnout`, `snow_golem_melts`,
  `ambient_sounds`, `creature_world_gen_spawn_probability`, `ambient_particles`,
  `water_fog_end_distance`, `music_volume`, `can_pillager_patrol_spawn`). So in the overworld
  the bed rule is constant: **a bed sleeps when dark and always sets the spawn; a straw bed
  sleeps when dark, never sets the spawn, and is destroyed when its sleeper leaves it.**

### 1.4 What is "dark" (`Level.isDarkOutside`)

D7:Level:

- `isDarkOutside()` (@0-22) = `!dimensionType.hasFixedTime() && !isBrightOutside()`.
- `isBrightOutside()` (@0-23) = `!hasFixedTime() && skyDarken < 4`.
- Hence **dark ⇔ `skyDarken >= 4`** (the overworld's `has_fixed_time` is absent and defaults
  to false: D:DimensionType `lambda$createDirectCodec$0`@0-15).
- `updateSkyBrightness()` (@0-25):
  `skyDarken = (int)(15.0f - environmentAttributes().getDimensionValue(SKY_LIGHT_LEVEL))`
  (`fsub`, then `f2i`, truncation toward zero).
- It is called in `ServerLevel.tick`@206 (D:ServerLevel), every level tick, after the sleep
  check and before `tickTime` (section 4.3). `skyDarken` is a field: between two level ticks
  (when packets are handled) it holds the value computed at the last level tick, with the
  clock as it was then.

`SKY_LIGHT_LEVEL` (D:EnvironmentAttributes @787-824): `gameplay/sky_light_level`, FLOAT,
default **15.0**, range [0, 15], `notPositional`, `syncable`. Its value is computed by the
attribute system (section 7.3) from: the default 15.0, the dimension's constant (the
overworld sets none), the `day` timeline's track (`multiply` by a keyframed factor) and the
weather layer. The formula, with float (f32) arithmetic throughout:

```
t      = floorMod(overworldClock.totalTicks, 24000)            // KeyframeTrackSampler.loopTicks
factor = segment lookup over the keyframes 133:1.0, 11867:1.0, 13670:0.26666668, 22330:0.26666668
         with wrap-around segments [-1670 → 133] and [22330 → 24133]   (section 7.4)
         alpha = (float)(t - from) / (float)(to - from); factor = from_v + alpha * (to_v - from_v)
v      = 15.0f * factor                                         // FloatModifier.MULTIPLY
// weather (only if rain > 0): r = rainLevel, th = thunderLevel * rainLevel
if (r - th) > 0: v = lerp(r - th, v, lerp(0.3125,     v, 4.0))
if th > 0:       v = lerp(th,     v, lerp(0.52734375, v, 4.0))  // lerp(a, s, e) = s + a*(e - s)
v      = clamp(v, 0, 15)                                         // sanitizeValue (range)
skyDarken = (int)(15.0f - v);  dark = skyDarken >= 4
```

Computed exactly in f32 over a whole day (`t` = day time, `floorMod(totalTicks, 24000)`):

| weather | `skyDarken` | dark (sleep allowed) |
|---|---|---|
| clear | 0 for t in [23970, 12030]; 1 at 12031; 2 at 12195; 3 at 12359; **4 at 12523**; … 11 from 13670 to 22330; 10 at 22331 … **4 up to 23477**; 3 at 23478; 0 from 23970 | **12523 ≤ t ≤ 23477** |
| raining fully (rain 1, thunder 0) | noon 11.5625 → 3 | 12002 ≤ t ≤ 23998 |
| thundering fully (rain 1, thunder 1) | noon 9.19921875 → 5 | all day |

The clear-weather steps fall every 164 ticks from 12031 (v = 15·factor drops one whole level
per 1803/11 ≈ 163.9 ticks). The middle of the night gives v = 4.0 exactly, `skyDarken` 11.
At t = 0 (the morning a sleep moves to), v = 14.1886, `skyDarken` 0: bright.

Weather in Clustine is always clear (n7-plan 11.6), so only the first row matters now; the
other two are here because a thunderstorm makes sleep possible by day in the game.
`rainLevel`/`thunderLevel` are `Level.getRainLevel(1.0)` = `rainLevel` and
`getThunderLevel(1.0)` = `thunderLevel * rainLevel` (D7:Level), which ramp gradually.

### 1.5 The checks of `ServerPlayer.startSleepInBed` (D:ServerPlayer @2727)

Arguments: the block, the head's state, the bed rule, the head's position. In this order:

1. @0-10: `direction = headState.FACING`.
2. @12-32: if `isSleeping()` or `!isAlive()`: `OTHER_PROBLEM` (no message).
3. @33-51: `canSleep = bedRule.canSleep(level)`, `canSetSpawn = bedRule.canSetSpawn(level)`,
   both evaluated now (1.4).
4. @53-70: if `!bedInRange(headPos, direction)`: `TOO_FAR_AWAY`.
   `bedInRange` (@2901) = `isReachableBedBlock(head) || isReachableBedBlock(head.relative(
   direction.opposite))` (the foot). `isReachableBedBlock(p)` (@2919): with
   `c = Vec3.atBottomCenterOf(p)` = (p.x+0.5, p.y, p.z+0.5): `|player.x - c.x| <= 3 &&
   |player.y - c.y| <= 2 && |player.z - c.z| <= 3`.
5. @71-88: if `bedBlocked(headPos, direction)`: `OBSTRUCTED`. `bedBlocked` (@2956): with
   `a = head.above()`, blocked unless `freeAt(a) && freeAt(a.relative(direction.opposite))`
   (above the head and above the foot). `freeAt(p)` (D:Player@3446) =
   `!level.getBlockState(p).isSuffocating(level, p)`.
6. @89-124: **if `canSetSpawn`**: `setRespawnPosition(new RespawnConfig(RespawnData.of(
   level.dimension(), headPos, getYRot(), getXRot()), false), true)` (section 2.1). This is
   before the darkness and monster checks: clicking a bed by day, or with monsters near,
   still sets the spawn.
7. @127-139: if `!canSleep`: `Either.left(bedRule.asProblem())`, whose message in the
   overworld is `block.minecraft.bed.no_sleep`.
8. @140-259: if not in creative mode: monsters near. `c = Vec3.atBottomCenterOf(headPos)`;
   the box is `AABB(c.x-8, c.y-5, c.z-8, c.x+8, c.y+5, c.z+8)`; `level.getEntitiesOfClass(
   Monster.class, box, m -> m.isPreventingPlayerRest(level, player))`; any → `NOT_SAFE`.
   `Monster.isPreventingPlayerRest` is `true` (D7:Monster@209); the only override in the jar
   is `ZombifiedPiglin`, which returns `isAngryAt(player, level)` (J). So every `Monster`
   subclass counts except a zombified piglin that is not angry at this player. Creative
   players skip the check; the respawn point is set before it either way.
9. @260-281: `Player.startSleepInBed(...)` (section 2.2), and if it succeeds:
   `lambda$startSleepInBed$1`: award the stat `block.getSleptInBedStatType()`
   (`SLEEP_IN_BED`, or `SLEEP_IN_STRAW_BED` for a straw bed) and, if `canSetSpawn`, trigger
   the advancement `SLEPT_IN_BED`.
10. @283-300: if `!level.canSleepThroughNights()` (`players_sleeping_percentage > 100`,
    D:ServerLevel@1199): overlay `sleep.not_possible`. Sent after step 9 whatever its result.
11. @303-307: `level.updateSleepingPlayerList()` (section 4.2), then return step 9's result.

Steps 2, 4, 5 and 7–8 return before steps 9–11: those failures send no `sleep.not_possible`
and do not update the sleep status.

### 1.6 The problems and messages

`Player$BedSleepingProblem` (record of one `Component message`, D:Player$BedSleepingProblem
static init):

| problem | message key | English (jar's en_us.json) |
|---|---|---|
| `TOO_FAR_AWAY` | `block.minecraft.bed.too_far_away` | You may not rest now; the bed is too far away |
| `OBSTRUCTED` | `block.minecraft.bed.obstructed` | This bed is obstructed |
| `OTHER_PROBLEM` | null (nothing is shown) | — |
| `NOT_SAFE` | `block.minecraft.bed.not_safe` | You may not rest now; there are monsters nearby |
| bed rule's `asProblem()` | `block.minecraft.bed.no_sleep` (overworld) | You can sleep only at night or during thunderstorms |

Also: `block.minecraft.bed.occupied` "This bed is occupied" (1.2 step 5), `sleep.not_possible`
"No amount of rest can pass this night" (1.5 step 10), `block.minecraft.set_spawn` "Respawn
point set" (2.1). All of them are translatable components with no arguments; all are sent on
the **action bar** except `block.minecraft.set_spawn`, which goes to **chat**.

---

## 2. Starting to sleep

### 2.1 The respawn point

- `ServerPlayer$RespawnConfig` (D) is a record `(respawnData: LevelData.RespawnData, forced:
  boolean)`; codec: `RespawnData.MAP_CODEC` fields plus `forced` (optional, default false);
  it is saved under the player's key `respawn` (D:ServerPlayer `readAdditionalSaveData`@124).
- `LevelData$RespawnData` (D) is `(globalPos: GlobalPos, yaw: float, pitch: float)`;
  `RespawnData.of(dimension, pos, yaw, pitch)` (@0-27) stores `pos.immutable()`,
  `Mth.wrapDegrees(yaw)` and `Mth.clamp(pitch, -90, 90)`. Codec fields: `yaw`, `pitch` and
  the global position's (**Not read**: the names of `GlobalPos.MAP_CODEC`'s fields, which are
  `dimension` and `pos` in the game's saves as far as known).
- A bed sets `RespawnConfig(RespawnData.of(overworld, headPos, player.yRot, player.xRot),
  forced = false)` (1.5 step 6). The position is the **head**.
- `setRespawnPosition(config, sendMessage)` (D:ServerPlayer@4940): if `sendMessage` and the
  new config is not null and `!config.isSamePosition(old)`, `sendSystemMessage(
  SPAWN_SET_MESSAGE)` = chat `block.minecraft.set_spawn` (static init @5982). Then store.
  `isSamePosition` compares only the `GlobalPos` (dimension and block), not the angles. So
  the first click on a bed says "Respawn point set"; clicking the same bed again (or its
  other half, which resolves to the same head) says nothing in chat but does store the new
  yaw and pitch.
- When the player respawns (outside this step's scope, named so the state is complete):
  `findRespawnAndUseSpawnBlock` (D:ServerPlayer@2286@161-223) uses a bed only if the block at
  the stored position is an `AbstractBedBlock` and its bed rule's `canSetSpawn` holds now; the
  place is `findStandUpPosition(PLAYER, level, pos, bedFacing, respawnData.yaw)` (3.3), and
  the respawn yaw looks at the bed (`RespawnPosAngle.calculateLookAtYaw`). **Not read**:
  what happens when no position is found (`findRespawnPositionAndUseSpawnBlock`@2122, the
  "no respawn block" game event and `block.minecraft.spawn.not_valid`).

### 2.2 Lying down

`Player.startSleepInBed` (D:Player@3211): `if (!startSleeping(headPos)) return
Either.left(OTHER_PROBLEM); sleepCounter = 0; return Either.right(Unit)`.

`ServerPlayer.startSleeping(pos)` (D:ServerPlayer@2869) calls `LivingEntity.startSleeping`
(the `invokespecial` resolves there; neither `Player` nor `Avatar` defines it) and, if it
succeeded, resets the stat `TIME_SINCE_REST` and calls `connection.teleport(x, y, z, yRot,
xRot)` with the new position and unchanged angles.

`LivingEntity.startSleeping(pos)` (D:LivingEntity@10398), in order:

1. @0-33: the block at `pos` must be an `AbstractBedBlock`, else false.
2. @34-55: `h = bed.getSleepHeight(state, level, pos)`; empty → false.
3. @56-63: if a passenger, `stopRiding()`.
4. @67-74: `setPosToBed(h, pos)` (@10504): `setPos(pos.x + 0.5, pos.y + h + 0.125,
   pos.z + 0.5)`. A bed: **y + 0.6875**; a straw bed: y + 0.375.
5. @77-99: `level.setBlockAndUpdate(pos, state.setValue(OCCUPIED, true))` (the head; the
   foot follows by its shape update, 1.1).
6. @100-104: `setPose(Pose.SLEEPING)`.
7. @107-109: `setSleepingPos(pos)` = `entityData.set(SLEEPING_POS_ID, Optional.of(pos))`.
8. @112-116: `setDeltaMovement(Vec3.ZERO)`.
9. @119-121: `needsSync = true`; return true.

The yaw and pitch are not changed by lying down.

### 2.3 Synced data and dimensions

- `SLEEPING_POS_ID` is the seventh accessor `LivingEntity` defines (D:LivingEntity static
  init @45-117: living flags, health, effect particles, effect ambience, arrow count, stinger
  count, sleeping position) after `Entity`'s eight (D7:Entity: shared flags, air, custom name,
  custom name visible, silent, no gravity, pose, ticks frozen): **index 14**, serializer
  `OPTIONAL_BLOCK_POS` (id 11 in `EntityDataSerializers`' registration order, D7; Clustine has
  it as `entity_data_serializer::OPTIONAL_BLOCK_POS`): a boolean presence flag, then the
  position as a long.
- `DATA_POSE` is **index 6**, serializer `POSE` (id 20, D7:EntityDataSerializers), value
  `SLEEPING` = **2** (J:Pose: STANDING 0, FALL_FLYING 1, SLEEPING 2, SWIMMING 3, SPIN_ATTACK 4,
  CROUCHING 5, LONG_JUMPING 6, DYING 7, …).
- `isSleeping()` is `getSleepingPos().isPresent()` (@10391): the synced position is the
  sleeping state; the client derives its "in bed" screen from it (**Inferred**).
- The sleeping pose's dimensions are `LivingEntity.SLEEPING_DIMENSIONS` =
  `EntityDimensions.fixed(0.2, 0.2).withEyeHeight(0.2)` (static init @120-135; `Avatar` maps
  `SLEEPING` to it, J:Avatar).
- `Player.getDesiredPose()` (D:Player@615) returns `SLEEPING` first whenever `isSleeping()`,
  so `updatePlayerPose` keeps it.
- Saved: `sleeping_pos` (`BlockPos.CODEC`) when present (`addAdditionalSaveData`@156-166);
  but a `ServerPlayer` loaded asleep is woken at once (`readAdditionalSaveData`@113-121,
  `stopSleeping()`), so a port need not keep a sleep across a log-out.

### 2.4 What clients are sent when a player lies down

In the order the code produces them (the packets that carry block and entity changes are
sent when the game flushes them, **Inferred** from the usual tracker behaviour):

1. The block change of the head (`OCCUPIED=true`) and then the foot (from its shape update),
   to the players who see those chunks.
2. To the sleeper: `ClientboundPlayerPositionPacket` from `connection.teleport` (2.2), with
   the bed position.
3. To the sleeper, in chat, `block.minecraft.set_spawn` if the spawn moved (1.5 step 6,
   which runs before the lying down).
4. To everyone in the level, on the action bar, the sleep status (4.4), if it changed.
5. The entity data (pose 6 = 2, sleeping position 14 = the head) to every tracker and to
   the sleeper, and a position sync to the trackers because of `needsSync` (**Inferred**:
   `ServerEntity`'s handling of dirty data and `needsSync` was not read for this step).

---

## 3. While asleep

### 3.1 The sleep counter

`Player.sleepCounter` (int). In `Player.tick` (D:Player@287), every player tick:

- @44-76: if `isSleeping()`: `sleepCounter++`, then capped at 100 (`if > 100 then 100`).
- @137-165: else if `sleepCounter > 0`: `sleepCounter++`, and when it reaches 110 it is set
  to 0. (After a wake that is not immediate it is 100, so it runs 101…109 then 0: the
  client's wake-up fade; harmless for a port to keep.)
- `startSleepInBed` sets it to 0 (2.2); `stopSleepInBed(wakeImmediately, …)` sets it to 0 if
  `wakeImmediately`, else 100 (D:Player@3227@28-39).
- `isSleepingLongEnough()` (@3258) = `isSleeping() && sleepCounter >= 100`.

A player is a "deep" sleeper after 100 of their own ticks in bed (5 seconds).

Player ticks run in the connection phase: `ServerGamePacketListenerImpl.tick` →
`tickPlayer`@50 → `ServerPlayer.doTick` → `Player.tick` (D:ServerPlayer@1083), which
`MinecraftServer.tickChildren` reaches after all levels have ticked (J:MinecraftServer
`tickChildren`@113-233: levels, then `tickConnection`).

### 3.2 What wakes a player by itself

1. **Day (the bed rule)**, `Player.tick`@76-131, on the server, while asleep: if the block
   at `blockPosition()` (`getInBlockState()`) is an `AbstractBedBlock` and
   `bed.getBedRule(level, blockPosition()).canSleep(level)` is false:
   `stopSleepInBed(false, true)`. In the overworld: once `skyDarken < 4`, i.e. at day time
   23478 in clear weather.
2. **The bed is gone, or the player cannot interact**, `LivingEntity.tick`@145-167: if
   asleep and (`!canInteractWithLevel()` or `!checkBedExists()`): `stopSleeping()`.
   `canInteractWithLevel()` (J) = `isAlive() && !isRemoved() && !isSpectator()`.
   `checkBedExists()` (@10529, `lambda$checkBedExists$0`) = the block at the sleeping
   position is an `AbstractBedBlock` and its `getSleepHeight` is present. For players
   `stopSleeping()` is `Player.stopSleeping()` = `stopSleepInBed(true, true)` (D:Player@3250).
3. **Damage**: `LivingEntity.hurtServer`@42-50 calls `stopSleeping()` once the damage passed
   the invulnerability and fire-resistance checks.
4. **Death**: `LivingEntity.die`@35-43 calls `stopSleeping()`.
5. **Teleport**: `ServerPlayer.teleportTo`@0-10: `stopSleepInBed(true, true)`.
6. **Disconnect**: `ServerPlayer.disconnect`@9-19: `stopSleepInBed(true, false)` (no sleep
   status update from this call; the level's `onTrackingEnd` updates it when the player
   leaves the level).
7. **Load**: 2.3.
8. **The leave-bed input**: section 5.
9. **The night skipped**: section 4.3 (`stopSleepInBed(false, false)` for everyone asleep).
10. On the first tick after loading, `LivingEntity.baseTick`@7-24 puts a sleeper back onto
    its bed (`setPosToBed`) or wakes them if the bed is gone (only matters for loaded
    sleepers, which players never are).

### 3.3 `stopSleepInBed(wakeImmediately, updateLevelForSleepingPlayers)`

`ServerPlayer.stopSleepInBed` (D:ServerPlayer@2977):

1. @0-24: if `isSleeping()`: `ClientboundAnimatePacket(player, 0)` (action 0 =
   `WAKE_UP` in 26.3; D7:ClientboundAnimatePacket) to the trackers and the player
   (`sendToTrackingPlayersAndSelf`).
2. @27-30: `Player.stopSleepInBed(wakeImmediately, update)`:
   1. `LivingEntity.stopSleeping()` (below);
   2. if `update` (and the level is a `ServerLevel`): `level.updateSleepingPlayerList()`
      (4.2), after the player is already awake, so they no longer count;
   3. `sleepCounter = wakeImmediately ? 0 : 100`.
3. @33-64: `connection.teleport(x, y, z, yRot, xRot)`: a `ClientboundPlayerPositionPacket`
   with the stand-up position and the new angles.

`LivingEntity.stopSleeping()` (D:LivingEntity@10543), in order:

1. If the sleeping position is in a loaded chunk (`LevelReader.hasChunkAt`, J bootstrap 28),
   `lambda$stopSleeping$0` (@11255):
   1. If the block there is an `AbstractBedBlock`: `direction = state.FACING`;
      `level.setBlockAndUpdate(pos, state.setValue(OCCUPIED, false))` (the head; the foot
      follows);
   2. `bed.onStopSleeping(level, pos)` (D:AbstractBedBlock): if the bed rule at `pos` has
      `destroyOnLeave`, `destroyOnLeave(level, pos)`. `BedBlock`: nothing. `StrawBedBlock`:
      play `STRAW_BED_BREAK_LEAVE` (block sound source, volume 1, pitch 1) at the head and
      set the head to air; the foot then turns to air by its shape update. **Inferred**:
      what the foot's removal drops and shows (`spawnDestroyParticles` is overridden to level
      event 2014) was not traced.
   3. `standUp = AbstractBedBlock.findStandUpPosition(player type, level, pos, direction,
      yRot)` or else `(pos.x + 0.5, pos.y + 1 + 0.1, pos.z + 0.5)` (`lambda$stopSleeping$1`:
      above the head, plus 0.1);
   4. `v = (atBottomCenterOf(pos) - standUp).normalize()`; `yaw = (float) wrapDegrees(
      atan2(v.z, v.x) * 57.2957763671875 - 90.0)`: the player faces the bed;
   5. `setPos(standUp)`, `setYRot(yaw)`, `setXRot(0)`.
   If the block is no longer a bed, none of this happens: the player stays where they lay.
2. `setPose(STANDING)`, `setPos(position())` (the same place), `clearSleepingPos()`.

`findStandUpPosition(type, level, headPos, facing, yRot)` (D:AbstractBedBlock@432):

- `side = facing.getClockWise()`; if `side.isFacingAngle(yRot)` use `side.opposite()`
  (@0-26). `Direction.isFacingAngle(yRot)` (J:Direction): `r = yRot * 0.017453292f`;
  `x = -Mth.sin(r)`, `z = Mth.cos(r)` (the game's sine table); true if
  `normal.x * x + normal.z * z > 0`. The yaw is the one the player had when they lay down:
  movement packets are not applied while asleep (8.2).
- If the block below the head is a bed (bunk bed, `isBunkBed`), the bunk order below;
  otherwise the offsets `bedStandUpOffsets(facing, side)` = the ten "surround" offsets then
  the two "above" offsets, tried all with `checkDangerous = true`, then all again with
  `false` (@46-85).
- Surround offsets (`bedSurroundStandUpOffsets(d = facing, s = side)`, @704), as (dx, dz)
  from the head, in order:
  `s`, `s - d`, `s - 2d`, `-2d`, `-s - 2d`, `-s - d`, `-s`, `-s + d`, `d`, `s + d`.
  Above offsets (`bedAboveStandUpOffsets(d)`): `(0, 0)` (on the head itself), `-d` (on the
  foot).
- Bunk order (`findBunkBedStandUpPosition`@481): surround at the head's Y with danger
  checks; surround one block lower with checks; above with checks; surround without; surround
  lower without; above without.
- Each candidate `(head.x + dx, head.y, head.z + dz)` goes to
  `DismountHelper.findSafeDismountLocation(type, level, p, checkDangerous)` (D5:DismountHelper
  @292), which returns a position or null:
  1. if `checkDangerous` and `type.isBlockDangerous(state at p)`: null;
  2. `floor = getBlockFloorHeight(nonClimbableShape(p), () -> nonClimbableShape(p.below()))`
     (J:BlockGetter: the shape's max Y if the shape at `p` is not empty; else the max Y of
     the shape below minus 1 if that is ≥ 1, else −∞). `nonClimbableShape` is empty for a
     block in `#climbable` or an open trapdoor, else the collision shape;
  3. not `isBlockFloorValid(floor)` (finite and `< 1`): null;
  4. if `checkDangerous` and `floor <= 0` and the block below is dangerous: null;
  5. `v = (p.x + 0.5, p.y + floor, p.z + 0.5)`; the type's standing box at `v` must have no
     block collisions;
  6. for the player type, neither `p` nor `p.above()` may be in `#invalid_spawn_inside`;
  7. the box must be inside the world border.
  **Not read**: `EntityType.isBlockDangerous` (the player's list of dangerous blocks) and the
  player type's box (0.6 × 1.8 as everywhere else in Clustine, **Inferred**).

---

## 4. The level skipping the night

### 4.1 `SleepStatus` (D:SleepStatus)

Two counters, `activePlayers` and `sleepingPlayers`, updated only by `update(players)`:

- `update` (@0-121): recount over the level's players: every player that is **not a
  spectator** adds 1 to `active`, and if also `isSleeping()` 1 to `sleeping`. Returns
  "changed" = `(oldSleeping > 0 || newSleeping > 0) && (oldActive != newActive ||
  oldSleeping != newSleeping)`.
- `sleepersNeeded(pct)` (@0-17) = `max(1, Mth.ceil((float)(activePlayers * pct) / 100.0f))`
  (integer product, then float division, then ceiling).
- `areEnoughSleeping(pct)` = `sleepingPlayers >= sleepersNeeded(pct)` (the stored count).
- `areEnoughDeepSleeping(pct, players)` = the number of players in the level's list (all of
  them; spectators cannot be asleep anyway) with `isSleepingLongEnough()` (J bootstrap 0 is
  `Player.isSleepingLongEnough`), counted live, `>= sleepersNeeded(pct)`.
- `amountSleeping()` = `sleepingPlayers`; `removeAllSleepers()` sets `sleepingPlayers = 0`
  (not `activePlayers`).

The game rule (D:GameRules static init @525-537): `players_sleeping_percentage`, integer,
category PLAYER, **default 100**, minimum 0. `canSleepThroughNights()` = rule `<= 100`.
Examples: one player → 1 needed; two players at 100 % → 2; three at 50 % → ceil(1.5) = 2;
rule 0 → 1.

### 4.2 `updateSleepingPlayerList` and when it runs

`ServerLevel.updateSleepingPlayerList()` (D:ServerLevel@1282): if the level has players and
`sleepStatus.update(players)` says changed, `announceSleepStatus()`.

Called from: `ServerPlayer.startSleepInBed`@307 (every bed attempt that reaches step 9, even a
failed one), `Player.stopSleepInBed` when `updateLevelForSleepingPlayers` (D:Player@3227@14-25),
`ServerLevel$EntityCallbacks.onTrackingStart` and `onTrackingEnd` for a `ServerPlayer` (a
player entering or leaving the level, J), and `ServerPlayerGameMode.changeGameModeForPlayer`@91
(a change of game mode, J).

### 4.3 The check in `ServerLevel.tick` (D:ServerLevel@426)

Each level tick, in this order:

1. @9-13: `environmentAttributes().invalidateTickCache()`.
2. @27-61: if the tick rate runs normally: world border tick, `advanceWeatherCycle()`.
3. @67-83: `pct = players_sleeping_percentage`.
4. @85-110: if `sleepStatus.areEnoughSleeping(pct) && sleepStatus.areEnoughDeepSleeping(pct,
   players)`:
   1. @113-170: if the game rule `advance_time` is true (default true, D:GameRules@6-26) and
      the dimension has a default clock (the overworld's is `minecraft:overworld`):
      `server.clockManager().moveToTimeMarker(overworldClock, WAKE_UP_FROM_SLEEP)` (4.5);
   2. @171: `wakeUpAllPlayers()` (@731): `sleepStatus.removeAllSleepers()`, then every
      player of the level with `isSleeping()` gets `stopSleepInBed(false, false)`
      (`lambda$wakeUpAllPlayers$0`): woken, counter 100, no sleep status update and so no
      announcement;
   3. @175-202: if the game rule `advance_weather` is true (default true) and it is raining:
      `resetWeatherCycle()` (@1634): rain time 0, raining false, thunder time 0, thundering
      false.
5. @205: `updateSkyBrightness()` (1.4), with the clock after the jump.
6. @209-214: if the tick rate runs normally, `tickTime()` (game time + 1, scheduled
   functions).
7. Then block ticks, entities and so on.

Notes:

- The sleep check is not inside the tick-rate condition: it runs even when ticking is frozen
  (only the clocks and game time stop).
- `areEnoughSleeping` uses the counters from the last `update`; `areEnoughDeepSleeping`
  counts now. The sleepers' counters advance in the connection phase after the levels
  (3.1), so a single player who lies down between ticks has `sleepCounter = k` at the level
  check of the (k+1)-th server tick after; the night is skipped in the level tick of the
  **101st** server tick after the bed was used (counter 100).
- When the clock is already exactly at a multiple of 24000, `moveToTimeMarker` does not move
  it (4.5), but everyone is still woken.

Within one server tick (J:MinecraftServer `tickChildren`): (a) `clockManager.tick()` if the
tick rate runs normally (@43-69), (b) every 20 ticks `forceGameTimeSynchronization()`
(@75-99), (c) each level's `tick` (above), (d) the connection phase: packets were already
handled between ticks, and here the players tick (3.1).

### 4.4 `announceSleepStatus` (D:ServerLevel@1214)

1. If `!canSleepThroughNights()` (rule > 100): nothing.
2. If the server is single-player and not published to LAN: nothing (never for a dedicated
   server).
3. `message = areEnoughSleeping(pct) ? translatable("sleep.skipping_night") :
   translatable("sleep.players_sleeping", amountSleeping(), sleepersNeeded(pct))` (the two
   arguments are `Integer`s; English "%s/%s players sleeping").
4. Sent with `sendOverlayMessage` (the action bar) to **every player in the level's player
   list**, spectators included.

It runs only when `update` reports a change (4.2). Consequences a player sees:

- Alone: lying down shows "Sleeping through this night" (1 of 1 needed).
- Two players, rule 100: the first to lie down makes both see "1/2 players sleeping"; when
  the second lies down both see "Sleeping through this night".
- Leaving the bed by the button with nobody else asleep: "0/2 players sleeping" (old
  sleeping 1, new 0, counts changed).
- The skip itself sends no status message: `removeAllSleepers` sets the count to 0 before
  the players are woken without updates, so the next `update` sees 0 → 0.
- A player joining, leaving or changing game mode while someone sleeps re-announces with the
  new numbers.

### 4.5 `ServerClockManager.moveToTimeMarker` (D:ServerClockManager)

- `moveToTimeMarker(clock, marker)` → `modifyClock(clock, lambda$moveToTimeMarker$0)`:
  1. look the marker up in the clock's markers; none → `NO_TIME_MARKER_FOUND`;
  2. if `marker.occursAt(totalTicks)` → `NOT_MOVED` (no change);
  3. else `totalTicks = marker.resolveTimeToMoveTo(totalTicks)`, `partialTick = 0`, `MOVED`.
- `ClockTimeMarker` (D:ClockTimeMarker), for a marker with `ticks = m` and period `P`:
  - `occursAt(t)` = `m == t % P` (Java `lrem`);
  - `resolveTimeToMoveTo(t)` = `t + durationToNext(P, t % P, m)`, where `durationToNext(P,
    cur, m)` = `d = m - cur; d > 0 ? d : P + d`.
- `wake_up_from_sleep` is `m = 0`, `P = 24000` (JSON:timeline/day.json; the period is the
  timeline's `period_ticks`, `Timeline.registerTimeMarkers`@55-77). So the new total is
  **`t + (24000 - t % 24000)`**, the next multiple of 24000 (and unchanged when `t % 24000 ==
  0`). Partial tick 0.
- `modifyClock` (@0-101), after the change (also after `NOT_MOVED`):
  1. `packet = ClientboundSetTimePacket(overworldLevel.getGameTime(), Map.of(clock,
     instance.packNetworkState(server)))`, sent to **all players of the server**
     (`PlayerList.broadcastAll`);
  2. `setDirty()`;
  3. every level's `environmentAttributes().invalidateTickCache()`.

The game time (the level's tick count) does not change; only the overworld clock jumps.

---

## 5. The leave-bed input

- `ServerboundPlayerCommandPacket` (D): `(id: VarInt entity id, action: VarInt enum ordinal,
  data: VarInt)`; `Action.STOP_SLEEPING` is ordinal 0 (D:ServerboundPlayerCommandPacket$Action
  static init: STOP_SLEEPING, START_SPRINTING, STOP_SPRINTING, START_RIDING_JUMP,
  STOP_RIDING_JUMP, OPEN_INVENTORY, START_FALL_FLYING). Clustine already decodes it
  (`player_command::STOP_SLEEPING = 0`).
- `ServerGamePacketListenerImpl.handlePlayerCommand` (D@5213): ignored until the client has
  loaded (`hasClientLoaded`); `player.resetLastActionTime()`; for `STOP_SLEEPING` (case
  @102-132): if `player.isSleeping()`: `player.stopSleepInBed(false, true)` (3.3: animation,
  awake, sleep status updated and announced, counter 100, position packet), then
  `awaitingPositionFromClient = player.position()`. If the player is not asleep, nothing.
- The packet's entity id is not checked against the player (**Inferred** from the handler,
  which never reads it).

---

## 6. `ClientboundSetTimePacket` and the clocks' states

### 6.1 The packet (D:ClientboundSetTimePacket, D:ClockNetworkState, D:WorldClock)

Record `(gameTime: long, clockUpdates: Map<Holder<WorldClock>, ClockNetworkState>)`; codec,
in order:

1. `gameTime`: `ByteBufCodecs.LONG`, 8 bytes big-endian;
2. `clockUpdates`: `ByteBufCodecs.map(…)`: a VarInt count, then for each entry the key
   `WorldClock.STREAM_CODEC` = `ByteBufCodecs.holderRegistry(WORLD_CLOCK)` (the VarInt id of
   the clock in the `minecraft:world_clock` registry as sent to the client) and the value
   `ClockNetworkState.STREAM_CODEC`: `totalTicks` VarLong, `partialTick` float, `rate` float.

Clustine's `SetTime` and `tell_time` (services/edge/src/fanout.rs) already write this shape.

### 6.2 The clock's state

`ServerClockManager$ServerClockInstance` (D): `totalTicks` long, `partialTick` float,
`rate` float (initially 1.0), `paused` bool. Saved as `ClockState` (`total_ticks`,
`partial_tick` default 0, `rate` default 1, positive, `paused` default false) in the saved
data `minecraft:world_clocks`.

- `tick()` (@0-50), called for every clock by `ServerClockManager.tick()` when the game
  rule `advance_time` is true, once per server tick before the levels: if not paused,
  `partialTick += rate; whole = Mth.floor(partialTick); partialTick -= whole; totalTicks +=
  whole`. With rate 1: `totalTicks + 1` each tick, partial 0.
- `packNetworkState(server)` (@0-61): `ClockNetworkState(totalTicks, partialTick, (paused ||
  !advance_time) ? 0.0f : rate)`: the rate sent is **0** when the clock is paused or time
  does not advance, so the client's clock stands still.
- `addTicks(n)` clamps at 0 (`max(total + n, 0)`); `setTotalTicks(t)` zeroes the partial tick.

Both registered clocks (`minecraft:overworld`, `minecraft:the_end`; JSON:world_clock/*.json
are `{}`) tick on the server whatever levels exist.

### 6.3 When the server sends it

| when | what | to whom | source |
|---|---|---|---|
| every 20 server ticks (`tickCount % 20 == 0`) | game time of the overworld, **empty map** | all players | J:MinecraftServer `tickChildren`@75-99, `forceGameTimeSynchronization`@13-34 |
| `PlayerList.sendLevelInfo` (join, respawn, change of dimension) | `createFullSyncPacket()`: game time and **every** clock's state (overworld and the_end) | that player | J:PlayerList `sendLevelInfo`@31, D:ServerClockManager `createFullSyncPacket` |
| a game rule changes (`MinecraftServer.onGameRuleChanged`@226; **Inferred**: for `advance_time`) | full sync | **Not read** (presumably all players) | J |
| any `modifyClock`: a sleep's skip, `/time`, `addTicks`, `setPaused`, `setRate` | game time and **that clock only** | all players | D:ServerClockManager `modifyClock` |

For Clustine (n7-plan 8.4.3): the skip's packet goes out at once in the level tick that
skips, before the sleepers' wake packets; the full sync at join carries the_end's clock too
(Clustine sends only the overworld's, which a client in the overworld does not use the
other of, **Inferred** harmless); the rate is 0 when `advance_time` is false.

---

## 7. The overworld's clock and timelines

### 7.1 Data

- JSON:world_clock/overworld.json: `{}` (the clock has no settings).
- JSON:dimension_type/overworld.json: `default_clock: minecraft:overworld`,
  `timelines: #minecraft:in_overworld`, `has_skylight: true`, `has_ceiling: false`, no
  `has_fixed_time` (false). Its `attributes` are constant layers: `audio/ambient_sounds`,
  `audio/background_music`, `gameplay/bed_rule`, `gameplay/nether_portal_spawns_piglin: true`,
  `gameplay/respawn_anchor_works: false`, `gameplay/straw_bed_rule`,
  `visual/ambient_light_color: #0a0a0a`, `visual/cloud_color: #ccffffff`,
  `visual/cloud_height: 192.33`, `visual/fog_color: #c0d8ff`, `visual/sky_color: #78a7ff`.
- JSON:tags/timeline/in_overworld.json: `#minecraft:universal` (= `villager_schedule`),
  `day`, `moon`, `early_game`, in that order (the order the timeline layers are applied).
- JSON:timeline/day.json: `clock: minecraft:overworld`, `period_ticks: 24000`.
- JSON:timeline/moon.json: same clock, period 192000; tracks `surface_slime_spawn_chance`
  (maximum, constant ease) and `visual/moon_phase` (one phase per 24000 from `full_moon`).
- JSON:timeline/early_game.json: same clock, no period; `can_pillager_patrol_spawn` false
  from 0, true from 120000 (`and`).

### 7.2 Time markers (JSON:timeline/day.json `time_markers`, `ClockTimeMarkers`)

All on the overworld clock, period 24000:

| marker | ticks | `show_in_commands` |
|---|---|---|
| `minecraft:day` | 1000 | true |
| `minecraft:noon` | 6000 | true |
| `minecraft:night` | 13000 | true |
| `minecraft:midnight` | 18000 | true |
| `minecraft:roll_village_siege` | 18000 | false (written as a bare number) |
| `minecraft:wake_up_from_sleep` | 0 | false |

(`Timeline$TimeMarkerInfo`: `{ticks, show_in_commands (default false)}` or a bare number.)

### 7.3 How an attribute's value is built (the attribute system)

`EnvironmentAttributeSystem` (D):

- Layers per attribute, in the order added by `addDefaultLayers` (D:…$Builder): static first
  (the dimension's constant map, then the biome layer for attributes some biome sets), then
  dynamic (one time-based layer per timeline of the dimension's tag that has a track for the
  attribute, in tag order; then, if the level can have weather (sky light, no ceiling, not
  the End: D7:Level `canHaveWeather`), the weather layer for the attributes the weather maps
  touch).
- Baking (`bakeLayerSampler`@0-102): starting from the attribute's default, leading constant
  layers are folded into a base value; the rest are applied in order at each read.
- `getDimensionValue(attr)` (used for sky light): cached per level tick
  (`ValueSampler.getDimensionValue`, cleared by `invalidateTickCache`); computes base, then
  each layer (positional layers skipped), then `attribute.sanitizeValue` (the range clamp).
- A time-based timeline layer (D:AttributeTrackSampler `applyTimeBased`):
  `modifier.apply(value, argumentSampler.sample(clock.totalTicks()))`, the argument cached
  per tick id. The clock's **total ticks only** (no partial tick) on the server.
- `KeyframeTrackSampler` (J): segments baked from the keyframes: with a period `P` and
  keyframes `k0 … kn`, the segments are `[kn.ticks - P → k0]`, each consecutive pair, and
  `[kn → k0.ticks + P]`; without a period, only the consecutive pairs; one keyframe gives a
  constant. `sample(t)`: `t = P ? floorMod(t, P) : t`; the segment is the first whose
  `toTicks > t` (else the last); `t <= from` gives the from value, `t >= to` the to value,
  otherwise `alpha = (float)(t - from) / (float)(to - from)`, eased, then the argument's lerp.
- Easing (J:EasingType): default `linear` (the track's `ease` field, default `LINEAR`,
  J:KeyframeTrack), which is the identity; `constant` is `x → 0` (always the from value);
  the tracks with `cubic_bezier` easings are the visual angles.
- Lerp of a float argument: `LerpFunction.ofFloat()` = `Mth.lerp(alpha, from, to)` =
  `from + alpha * (to - from)` (J). Not-interpolated types (booleans, tri-states) use a step at
  alpha 1 for keyframes (`AttributeType.ofNotInterpolated`: keyframe `ofStep(1)`, state change
  `ofStep(0)`, spatial `ofStep(0.5)`, partial tick `ofStep(0)`), so a boolean track switches
  exactly at its keyframe tick.
- Float modifiers (D:FloatModifier, J bootstraps): `override` (no modifier field), `add`
  (`Float.sum`), `subtract` (`value - arg`), `multiply` (`value * arg`), `minimum`
  (`Math.min`), `maximum` (`Math.max`), `alpha_blend` (`Mth.lerp(arg.alpha, value,
  arg.value)`). Boolean modifiers: `and`, `nand`, `or`, `nor`, `xor`, `xnor`.
- The weather layer (D:WeatherAttributes `lambda$addLayer$0`): `th = thunderLevel`,
  `ro = rainLevel - th`; if the rain map has the attribute and `ro > 0`: `value =
  type.stateChangeLerp(ro, value, rainEntry.apply(value))`; then the same with the thunder
  map and `th`. For `sky_light_level`: rain `alpha_blend(4.0, 0.3125)`, thunder
  `alpha_blend(4.0, 0.52734375)`; float's state change lerp is `ofFloat`.

### 7.4 What the `day` timeline drives (JSON:timeline/day.json)

Gameplay attributes (what a server needs):

| attribute | default (D:EnvironmentAttributes) | modifier | keyframes (tick: value) | ease |
|---|---|---|---|---|
| `gameplay/sky_light_level` | 15.0 (0–15) | multiply | 133: 1.0, 11867: 1.0, 13670: 0.26666668, 22330: 0.26666668 | linear |
| `gameplay/monsters_burn` | false | or | 12542: false, 23460: true | step |
| `gameplay/bees_stay_in_hive` | false | or | 12542: true, 23460: false | step |
| `gameplay/creaking_active` | false | or | 12600: true, 23401: false | step |
| `gameplay/eyeblossom_open` | tri-state default (**Not read**) | override | 12600: true, 23401: false | step |
| `gameplay/cat_waking_up_gift_chance` | 0.0 | maximum | 362: 0.0, 23667: 0.7 | constant |
| `gameplay/turtle_egg_hatch_chance` | 0.002 | maximum | 21062: 1.0, 21905: 0.002 | constant |
| `audio/firefly_bush_sounds` | false | or | 12600: true, 23401: false | step |

With the period wrap, a two-keyframe boolean is the second value from its second keyframe to
the first keyframe of the next day, and the first value between. So in the overworld:
monsters burn in [23460, 12542) and not in [12542, 23460); bees stay in [12542, 23460);
creaking active and eyeblossoms open and firefly sounds in [12600, 23401). A `constant` ease
holds the from value over a segment and switches at its end: the cat gift chance is 0.7 in
[23667, 362) and 0 in [362, 23667); the turtle egg chance 1.0 in [21062, 21905) and 0.002
otherwise.

Sky light level in clear weather (the factor's segments, from 7.3):

| day time t | factor |
|---|---|
| [0, 133) | segment −1670 → 133, 0.26666668 → 1.0: `alpha = (t + 1670) / 1803` |
| [133, 11867] | 1.0 (v = 15) |
| (11867, 13670) | `1.0 + ((t − 11867) / 1803) · (0.26666668 − 1.0)` |
| [13670, 22330] | 0.26666668 (v = 4.0) |
| (22330, 24000) | `0.26666668 + ((t − 22330) / 1803) · (1.0 − 0.26666668)` |

The night thresholds of section 1.4 follow from this table with f32 arithmetic.

Visual attributes, all the client's business (named, with their modifiers, for completeness):
`visual/sky_color`, `fog_color`, `cloud_color` (multiply; white by day, `#000000`,
`#0f0f16`, `#ff191926` at night; keyframes 133, 11867, 13670, 22330), `sky_light_color`
(multiply; white, `#7a7aff`; 730, 11270, 13140, 22860), `sky_light_factor` (multiply; 1.0,
0.24; same ticks), `star_brightness` (maximum; twelve keyframes from 92 to 23758, 0 by day,
0.5 at night), `sunrise_sunset_color` (override; 32 keyframes), `sun_angle`, `moon_angle`,
`star_angle` (override; two keyframes at 6000 with a cubic Bézier `(0.362, 0.241, 0.638,
0.759)`, i.e. a full turn per day).

---

## 8. What else a player notices about sleeping within minutes

1. **Others see the sleeper lying in the bed** (pose and sleeping position, 2.3) and the bed
   occupied; another player clicking it gets "This bed is occupied".
2. **Movement is ignored while asleep**: in `handlePlayerPositionChange`
   (D:ServerGamePacketListenerImpl, the method at line 3108, @120-224), if the player is
   asleep and the packet's position is more than 1 block from the last good position
   (squared distance > 1), the server teleports them back to their current position
   (`teleport(x, y, z, yaw, pitch)` with the method's float arguments, **Inferred** to be the
   packet's angles); otherwise the packet is dropped without being applied. Either way the
   position, and (**Inferred**: rotation-only packets take the same path) the yaw, stay as
   they were. The "moved wrongly" check (@653-718) and the flying kick (`tickPlayer`@104-128)
   skip sleepers.
3. **The hitbox** shrinks to 0.2 × 0.2 at y + 0.6875 (2.2, 2.3).
4. **Parrots leave the shoulders** while asleep: `ServerPlayer.handleShoulderEntities`
   (@1446@16-58) removes them when falling more than 0.5, in water, flying, asleep or in
   powder snow.
5. **Waking faces the bed**, beside it on the side away from where the player looked
   (3.3).
6. **Damage wakes** (3.2).
7. **The night skip** jumps the sky at once (the set_time with the new total and partial 0),
   stops rain and thunder, and wakes everyone in the level; players who were in bed but not
   yet 100 ticks are woken too.
8. **A thunderstorm allows sleep by day** (1.4); rain alone extends the night to
   12002–23998.
9. **Straw beds** break when their sleeper gets up (3.3) and never set the spawn.
10. Named and left out: phantoms (they read the stat `TIME_SINCE_REST`, which lying down
    resets, 2.2; `ServerPlayer.doTick`@91-102 counts it up while awake); statistics
    (`SLEEP_IN_BED`, `SLEEP_IN_STRAW_BED`) and the `slept_in_bed` advancement trigger; the
    village siege marker; cats' morning gifts (`cat_waking_up_gift_chance`); villagers in
    beds (`kickVillagerOutOfBed`); the bed's explosion in other dimensions; the respawn
    anchor.

---

## 9. Clustine's state against this

- `WorldClock { game_time, day }` (crates/clustine-sim/src/api.rs): `day` is the overworld
  clock's `totalTicks`; with rate 1 and no `/time` the partial tick is always 0, so the
  pair is enough. A skip is `day = day + (24000 - day % 24000)` when `day % 24000 != 0`;
  `game_time` unchanged. The ratchet (`later`) keeps working because a skip only moves
  forward.
- "Dark" for a port: `skyDarken(day)` from 1.4 (clear weather), computed in f32 exactly as
  above, or a table: clear weather is dark for `12523 <= day % 24000 <= 23477`. The bed rule
  is constant in the overworld, so `can_sleep = dark`, `can_set_spawn = true` (bed), `false`
  (straw bed).
- A player's state needs: `sleeping_pos: Option<BlockPos>` (the head), `sleep_counter: u8`
  (0–110), the respawn config `(pos, yaw, pitch, forced=false)`. `PlayerFace.sleeping`
  already carries the counter for other regions' sleep checks (n7-plan 8.4.4).
- The edge's `tell_time` sends the full overworld clock only on a region change and the game
  time alone otherwise; the game sends the clock at once on a skip (to everyone), at join,
  and the game time alone every 20 ticks (6.3).

---

## 10. Points marked Inferred or Not read

1. **Not read**: the field names of `GlobalPos.MAP_CODEC` in the saved respawn config (2.1).
2. **Not read**: what a respawn does when the bed gives no stand-up position
   (`findRespawnPositionAndUseSpawnBlock`, `block.minecraft.spawn.not_valid`) (2.1).
3. **Inferred**: the order and recipients of the block-update, entity-data and
   `needsSync` packets when lying down and getting up (`ServerEntity` not read) (2.4).
4. **Inferred**: the client derives its in-bed screen and wake fade from the synced
   sleeping position and its own counter (2.3, 3.1).
5. **Inferred**: what the straw bed's foot drops and which particles show when it breaks on
   leaving (3.3).
6. **Not read**: `EntityType.isBlockDangerous` for players; **Inferred**: the player's
   standing box of 0.6 × 1.8 in the stand-up search (3.3).
7. **Inferred**: rotation-only movement packets are dropped while asleep like the others,
   so the yaw used for the stand-up side is the one at lying down (3.3, 8.2).
8. **Inferred**: `onGameRuleChanged`'s full time sync is for `advance_time` and goes to all
   players (6.3).
9. **Inferred**: sending only the overworld clock at join (where the game also sends
   the_end's) is harmless to a client in the overworld (6.3).
10. **Inferred**: the leave-bed packet's entity id is not checked (5).
11. **Not read**: `eyeblossom_open`'s tri-state default (7.4).
12. **Inferred**: `BlockState.isSuffocating` (used for "obstructed") is the game's usual
    suffocation predicate; not traced here (1.5 step 5).
13. **Inferred**: the angles the server snaps a sleeper back with are the packet's (8.2).

A helper written for this brief, not part of the repository:
`/tmp/claude-1000/-home-claude-clustine/5730d18b-3e13-41be-bbe3-15a8120b306f/scratchpad/jdis.py`
(`python3 -I jdis.py JAR CLASS [METHOD]`) prints a class's bytecode with offsets and its
bootstrap methods, read from the jar in memory.
