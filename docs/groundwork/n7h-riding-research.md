# N7h groundwork: riding a pig (and what a horse changes)

Read from the 26.3 `javap -c` dumps on 2026-10-20. Short names: `Pig` =
`n7-javap/net.minecraft.world.entity.animal.pig.Pig.txt`, `Steering` =
`n7-javap/…ItemBasedSteering.txt`, `Stick` = `n7-javap/…item.FoodOnAStickItem.txt`,
`Horse` = `n7-javap/…animal.equine.AbstractHorse.txt`; `Entity`, `LivingEntity`, `Mob`,
`Animal`, `Player`, `ItemStack`, `Items` are the `n7-javap` files; `SGPLI`
(`ServerGamePacketListenerImpl`), `ServerPlayer`, `PlayerList`, `ServerEntity` and
`ServerPlayerGameMode` are the `n5-javap` files (identical copies in n6/n7; n7d's differ
only in how constants print). Numbers are file lines. **Inferred** marks what was not
read; **Client** marks what runs only on the client, which is not in the jar.

**What of N5 applies unchanged** (n5-brief sections 0 to 2): `startRiding`'s checks,
`addPassenger`'s order, `removePassenger`'s cooldown, the passenger tick order and the
riding player's double tick, `rideTick` → `positionRider`, the authority rules (1.6),
`handleMoveVehicle` (2.3) with its "too quickly", "moved wrongly", refusal and floating
kick, `tickPlayer`'s `lastVehicle` seeding (2.2), the mount and dismount teleports, the
sneak dismount (2.4), `LivingEntity.dismountVehicle` (1.7), `ServerEntity`'s rider
branch (1.11.1), saving with the player (1.9) and the effective tracking range. A pig
or a horse is an ordinary root vehicle to all of it. What is new is below.

## 1. Mounting a pig

There is no `Saddleable` and no `SaddleItem` in 26.3 (no dumped class names either):
the saddle is equipment slot `SADDLE`. `Mob.isSaddled()` = `hasValidEquippableItemForSlot
(SADDLE)` (Mob 2080–2110: an item in the slot that `isEquippableInSlot` there). Putting
a saddle on is n7-brief 4.9.3 (checked; `Equippable.equipOnTarget`, guaranteed drop, the
pig's equip sound `entity.pig.saddle`, Pig 394–406). `Pig.canUseSlot(SADDLE)` =
`isAlive() && !isBaby()` (Pig 360–378): a piglet cannot be saddled.

The click reaches `Mob.interact` (Mob 2727–2800): dead → `PASS`; a name tag or spawn
egg first; then `LivingEntity`/`Entity.interact` (leads and shears: a lead in hand
leashes the pig, shears take the saddle off, both before any mounting); only on `PASS`
does `mobInteract` run. `Pig.mobInteract` (Pig 303–358):

```
food = isFood(player.getItemInHand(hand))          // #pig_food: carrot, potato, beetroot
if !food && isSaddled() && !isVehicle() && !player.isSecondaryUseActive():
    if server: player.startRiding(this)            // result ignored
    return SUCCESS
r = Animal.mobInteract(player, hand)                // feeding, breeding
if r.consumesAction(): return r
if isEquippableInSlot(stack, SADDLE): return stack.interactLivingEntity(player, this, hand)
return PASS
```

So anyone not sneaking, holding anything but pig food (an empty hand, a carrot on a
stick), mounts a saddled, unridden pig; `SUCCESS` is returned even when `startRiding`
refuses (e.g. the `boardingCooldown` of a non-player, n5-brief 1.0 item 6).

`startRiding` on a living vehicle: `Entity.canAddPassenger` = `passengers.isEmpty()`
(Entity 7180; no override in `LivingEntity`, `Mob`, `Animal` or `Pig`): one rider.
`Mob.startRiding` (Mob 3088–3101) only matters when the mob is the rider (drops its
leash). `ServerPlayer.startRiding` (ServerPlayer 5282–5329) adds, for a
`LivingEntity` vehicle, `PlayerList.sendActiveEffects(vehicle, connection)`: the pig's
effects go to the rider's screen; `ServerPlayer.removeVehicle` (5330–5384) sends a
`RemoveMobEffect` for each at dismount. `LivingEntity.sendEffectToPassengers` (2505–)
sends later effects to player passengers.

## 2. Who controls a ridden pig

`Pig.getControllingPassenger` (Pig 128–150): if `isSaddled()` and `getFirstPassenger()`
is a `Player` and `player.isHolding(CARROT_ON_A_STICK)`, that player; else
`Animal`'s, which is `Mob`'s (Mob 306–326: the first passenger if the pig is not
`NoAi`, it is a `Mob`, and it `canControlVehicle()`). `isHolding(Item)` is the main
hand **or** the off hand (LivingEntity 5753–5780). "First passenger" is
`passengers[0]` (Entity 10173–10186); a pig has only one.

So control follows the held item **every time it is asked**: switching the hotbar slot
away from the stick hands the pig back to the server (AI, server physics) at once, and
back again. The edge's "who drives" must be re-judged on `SetCarriedItem` and on any
inventory change of either hand, not only at mount.

## 3. Who simulates it, and the server's tick

26.3 has no `isControlledByLocalInstance`; its successors are in Entity 10322–10408
(n5-brief 1.6). No class from `Pig` up overrides `getMoveSimulationType` (only `Player`,
items, falling blocks, TNT, orbs do), so a pig is `AUTHORITATIVE_SIDE`, and with a
player in control `isClientAuthoritative()` is true (`Player` 3114–3117):
**a controlled pig is the client's, exactly as a boat is.** On the server
`canSimulateMovement()` and `isEffectiveAi()` are false (`Mob.isEffectiveAi`, Mob
3106–3120, adds `!isNoAi()`).

The server's `LivingEntity.aiStep` (8214–8700) for a controlled pig, in order:
1. not interpolating and not simulated → `deltaMovement *= 0.98` (offsets 17–42);
2. `isImmobile()` (only when dying, 5965) else `isEffectiveAi()` false → **no
   `serverAiStep`**: no goals, sensing, navigation, look control (offsets 207–274);
3. the controlling passenger is a `Player` and the pig `isAlive()` → `travelRidden
   (player, (xxa, yya, zza))` (offsets 551–587), which (7051–7077) calls
   `getRiddenInput`, then `tickRidden`, then, not simulated, `setDeltaMovement(ZERO)`.

`Pig.tickRidden` (439–466), run on the server too:
```
setRot(player.getYRot(), player.getXRot() * 0.5f)     // each % 360f (Entity 793–805)
yRotO = yBodyRot = yHeadRot = getYRot()
steering.tickBoost()
```
`getRiddenInput` (468–476) = `(0, 0, 1)` always; `getRiddenSpeed` (478–491) =
`(float)(movement_speed · 0.225 · (double) boostFactor())` = `0.05625 · factor` for the
default 0.25. The last two are used only where the pig is simulated (**Client**).

Packets run before the tick, so the pig's rotation sent to others is the **rider's**
(yaw, half the pitch), overwriting the rotation `handleMoveVehicle` took from the
client. A pig ridden without the stick is the server's: AI on, it wanders with its
rider; the rider's client sends no `move_vehicle` (**Client**), and one that came
would get `handleMoveVehicle`'s "not controlling" correction.

`handleMoveVehicle` (SGPLI 669–1109) with a pig as root, beyond n5-brief 2.3:
- a `LivingEntity` root `onClimbable()` → `resetFallDistance()` before the move (855–864);
- the move's step height: `LivingEntity.maxUpStep()` (11025–11041) =
  `(float) STEP_HEIGHT`, raised to `max(…, 1.0f)` while the controlling passenger is a
  `Player`: 1.0 for a controlled pig (0.6 otherwise, **Inferred** default attribute);
- accepted moves call `doCheckFallDamage(dx, dy, dz, onGround)` (1020; Entity
  3959–3988) → `LivingEntity.checkFallDamage` (492–) → on landing
  `LivingEntity.causeFallDamage` (4469–4520), which first calls `Entity.causeFallDamage`
  → `propagateFallToPassengers` (Entity 4051–4086: **the rider takes
  `causeFallDamage(distance, multiplier, source)` with the pig's distance**), then hurts
  the pig by `calculateFallDamage`. A pig ridden off a cliff hurts both;
- the floating kick: `getMaximumFlyingTicks(pig)` = `ceil(80 · max(0.08 / g, 1))` = 80
  ticks with gravity 0.08 (**Inferred** attribute default; boats' 160).

## 4. The boost

`ItemBasedSteering` (whole class, Steering 1–118): constants `MIN_BOOST_TIME = 140`,
`MAX_BOOST_TIME = 700`; fields `boosting`, `boostTime` (neither synched nor saved), and
the pig's `DATA_BOOST_TIME` accessor.
- `boost(random)` (37–62): if `boosting` → false. Else `boosting = true; boostTime = 0;
  entityData.set(DATA_BOOST_TIME, random.nextInt(841) + 140)` → true. One draw from the
  **pig's** random (`Pig.boost`, 493–500), 140 to 980 ticks: the constants are not
  what the code uses.
- `tickBoost()` (64–82): `if (boosting && boostTime++ > boostTimeTotal()) boosting =
  false` (post-increment, compared before it).
- `boostFactor()` (84–106): `boosting ? 1.0f + 1.15f · Mth.sin((double)(boostTime /
  (float) total · 3.1415927f)) : 1.0f` (the product in float, widened for `sin`).
- `onSynced()` (27–35): `boosting = true; boostTime = 0`. `Pig.onSyncedDataUpdated`
  (152–168) calls it on the **client** only, when `DATA_BOOST_TIME` changes.
- `DATA_BOOST_TIME`: `EntityDataSerializers.INT`, default 0 (Pig 170–185, 775–780);
  index 18, before `DATA_VARIANT_ID` 19 (`PIG_VARIANT`) and `DATA_SOUND_VARIANT_ID` 20
  (`PIG_SOUND_VARIANT`) (n7-brief 4.9.2).

`tickBoost` runs only from `tickRidden`, i.e. only while a player controls the pig: a
boost pauses when the stick is put away. Each side counts its own `boostTime`.

`FoodOnAStickItem.use` (Stick 20–72); the carrot on a stick is `(PIG, 7)` with
durability 25 (Items 3804–3813, 9163–9172); the warped fungus one `(STRIDER, 1)`, 100.
```
client → PASS
v = player.getControlledVehicle()                    // vehicle whose controller is player
if player.isPassenger() && v is ItemSteerable && v.is(PIG) && v.boost():
    out = stack.hurtAndConvertOnBreak(7, FISHING_ROD, player, hand.asEquipmentSlot())
    return SUCCESS_SERVER.heldItemTransformedTo(out)
player.awardStat(ITEM_USED.get(this))                // only on this failing path
return PASS
```
Durability (ItemStack 744–937): `processDurabilityChange` gives 0 for a player with
`hasInfiniteMaterials()` (creative): **a creative player pays nothing**; otherwise
Unbreaking may lower it. `applyDamage` sets the damage and, once `damage >= 25`
(`isBroken`, 712), shrinks the stack and calls `onEquippedItemBroken` (the break
entity event 47/48, n7-brief table at 2813); `hurtAndConvertOnBreak` then returns a
fishing rod at damage 0. Unenchanted: three boosts leave damage 21, the fourth breaks it.
A click while boosting changes nothing but the stat.

What is sent: the new `DATA_BOOST_TIME` as `set_entity_data` to the pig's trackers (the
rider among them, whose client then boosts its own simulation); for a survival player
`ServerPlayerGameMode.useItem` (931–1040) sees the damage change, `setItemInHand`s the
result and calls `inventoryMenu.sendAllDataToRemote()` (the whole inventory); in
creative nothing changes, so it returns at offset 131 and sends nothing.

## 5. Getting off

`Pig` does not override `getDismountLocationForPassenger`; `Animal` does (Animal
545–656): `dir = getMotionDirection()` = `Direction.fromYRot(pig yRot)` (Entity
9955–9959); `offsets = DismountHelper.offsetsForDirection(dir)` (right of the pig's
facing, left, back-right, back-left, front-right, front-left, back, front: n5-brief
1.7); for each pose of `rider.getDismountPoses()` (player: standing, crouching,
swimming), for each offset: `p = blockPosition() + (ox, 0, oz)` (the pig's own block
height only); `floor = level.getBlockFloorHeight(p)` (the collision shapes at `p` and
below; climbables are **not** excluded, unlike the minecart's); valid floor and
`DismountHelper.canDismountTo(level, rider, rider.getLocalBoundsForPose(pose).move(
upFromBottomCenterOf(p, floor)))` (the pose's full width) → `rider.setPose(pose)`,
return that point. Nothing fits → `Entity`'s `(x, box.maxY, z)`: on top of the pig.
`AgeableMob`, `PathfinderMob`, `Mob` and `LivingEntity` add nothing between.

Sneaking: `Player.rideTick` (753–771), as for boats. Water: the **rider's**
`LivingEntity.baseTick` (offsets 205–389, lines ~775–867): with the rider's eyes in
water and no bubble column at them, if `getVehicle().dismountsUnderwater()`
(`#dismounts_underwater` lists the pig) → `stopRiding()`.

Hurt: nothing in `LivingEntity.hurtServer` (2794–3626) or `die` touches passengers;
the rider stays on a hurt or dying pig. A dying pig is `isImmobile`, `travelRidden`
needs `isAlive`, and it is still client-authoritative if the rider holds the stick, so
the server leaves it where the last accepted move put it. `tickDeath` (1129–1153), at
`deathTime >= 20`: entity event 60 and `remove(KILLED)`; `setRemoved` makes the rider
`stopRiding`, and `dismountVehicle` takes the removed-vehicle branch (`findFreePosition`
around `max(y, pig y)`, n5-brief 1.7). The saddle drops with the equipment (guaranteed
drop). A spear stab with dismounting (`stabAttack`, LivingEntity 7815–7900) unseats a
rider not in `#cannot_be_dismounted_by_item_usage`. Lightning (`thunderHit`, Pig
408–437; `ConversionType$1.convert` 12–114) moves the rider onto the new zombified
piglin: `stopRiding`, `boardingCooldown = 0`, `startRiding(newMob)`.

## 6. The seat

Pig passenger point `(0, 0.86875f, 0)` (`entity_types.rs` line 272:
`0x3febccccc0000000` = 0.8687499761581421); the player's vehicle point `(0, 0.6, 0)`
(line 330). No `positionRider`, `getPassengerAttachmentPoint` or `onPassengerTurned`
override from `Pig` up to `Entity`, so `Entity.positionRider` (6815–6843) with
`LivingEntity.getPassengerRidingPosition` (11043–11059): `position + attachment(index,
yRot)` from `getDimensions(pose)` = `getDefaultDimensions(pose).scale(getScale())`
(10265–). An adult pig puts the player's feet at `pig.y + 0.8687499761581421 − 0.6`,
x and z unchanged (the point has no horizontal part). **The pig does not touch the
rider's body rotation** (only the horse does, 9.2).

## 7. What clients are shown

As for boats (n5-brief 1.0 items 4, 5, 7; 1.11): `set_passengers` from the pig's
tracker when its list changes, at pairing, and to the rider at mount and dismount;
the rider gets only `MoveEntity.Rot`. The pig is not a passenger, so its own
`ServerEntity` takes the ordinary branch: position moves every 3 ticks (pig
`update_interval` 3), its yaw/pitch (the rider's, section 3), `RotateHead`, and its
zero motion, to every tracker but itself: **the rider is a tracker and is sent the
pig's moves** (its client ignores them for the vehicle it drives, **Client**). Its
tracking range becomes the rider's (32 chunks) while ridden. The saddle is equipment
(`set_equipment`, slot `SADDLE` 7); the boost is `set_entity_data` index 18.

## 8. Logging out on a pig

A pig ridden by exactly one player is not saved with its chunk (`Entity.shouldBeSaved`,
11340–11368). `PlayerList.remove` (575–680): `save(player)` (whose
`ServerPlayer.saveParentVehicle`, 550–583, writes `RootVehicle: {Attach: pig UUID,
Entity: the pig with its Passengers}` when the root `hasExactlyOnePlayerPassenger()`);
then, that still true, `player.stopRiding()` (the dismount, computed for nothing), every
entity of `root.getPassengersAndSelf()` `setRemoved(UNLOADED_WITH_PLAYER)`, `unRide()`,
ender pearls likewise, `removePlayerImmediately`. **The pig leaves the world with the
player**, and with two players aboard it would stay (a pig holds one). Each other
`save(player)` (autosave, **Inferred**) also writes `RootVehicle`, while the chunk never
holds the pig. At login `ServerPlayer.loadAndSpawnParentVehicle` (584–680) rebuilds it
with `addWithUUID` and seats the player with `ServerPlayer.startRiding(…, true, false)`
(teleport, effects, `set_passengers`); **its caller is not dumped** (no dumped class
invokes it; `PlayerList.placeNewPlayer`, 109–544, does not).

## 9. Horses: what differs (and n7-brief 5.15 checked)

1. **Mounting** (`AbstractHorse.mobInteract`, Horse 1535–1596): a ridden or baby horse
   → `Animal`'s; tamed and sneaking → inventory; an item → its interaction, then body
   armour; else `doPlayerRide` (1066–1090): stop eating and standing, **the player's
   yaw and pitch set to the horse's**, `startRiding`. `Horse.mobInteract` (`…equine.Horse.txt`
   284–345) feeds food and makes an untamed horse rear (`makeMad`) at other items.
   `addPassenger` (1782–1795) snaps the passenger's rotation to the horse's view.
2. **Control** (2420–2438): saddled and a `Player` first, **no stick and no taming
   check**. Taming gates it only through the saddle: `canUseSlot(SADDLE)` needs
   `isTamed()` (357–379). A saddled untamed horse (a command's) would obey
   (**Inferred** from the code). Taming is `RunAroundLikeCrazyGoal.tick` (106–145, AI
   while untamed): `nextInt(adjustedTickDelay(50))` = `nextInt(25)` (Goal 70–86) `== 0`,
   then for a player rider `nextInt(100) < temper` → `tameWithName`, else `temper += 5`,
   `ejectPassengers`, `makeMad`, entity event 6.
3. **Server tick while controlled**: as the pig (no AI, zero motion); `tickRidden`
   (1711–1767) sets `(yRot, xRot) = (rider yRot, rider xRot · 0.5f)` via
   `getRiddenRotation` and the same three copies; its jump branch runs only where
   `isLocalInstanceAuthoritative` (**Client**). No boost. `getRiddenInput` (1797–1839):
   zero while on the ground, no pending jump, standing and `!allowStandSliding`; else
   `(xxa · 0.5f, 0, zza ≤ 0 ? zza · 0.25f : zza)`; `getRiddenSpeed` = movement speed
   (1841–1847); flying speed `speed · 0.1f` (LivingEntity 7334–7347). 5.15 items 3, 4
   are right (`getPlayerJumpPendingScale`: `power >= 90 ? 1 : 0.4 + 0.4 · power / 90`;
   the forward kick only when `input.z > 0`).
4. **Jumps and rearing**: `player_command START_RIDING_JUMP` with `canJump()` (saddled)
   and `data > 0` → `handleStartJump` (2229–2238: `allowStandSliding`, `standIfPossible`,
   jump sound; SGPLI 5213–, offsets 135–180); `handleStopJump` does nothing. A hurt
   horse rears on `nextInt(3) == 0` (599–618).
5. **Inventory key**: `openCustomInventoryScreen` (803–825), server only, when the
   horse is unridden or ridden by that player, and tamed.
6. **Seat and body**: `positionRider` (2317–2333) sets the rider's `yBodyRot` to the
   horse's; the point adds `(0, 0.15·standAnimO·s, −0.7·standAnimO·s)` turned by
   `−yRot` (2646–2681). Horse passenger point 1.44375 (`entity_types.rs` 238).
7. **Fall** (495–534; attributes 760–783: safe fall 6, multiplier 0.5, step 1.0, jump
   0.7): land sound above 1 block; **if the horse takes no damage, the rider takes
   none either** (it returns before `propagateFallToPassengers`); the pig propagates
   always. 5.15 item 1 should say so.
8. **Correction to 5.15 item 7**: the dismount in water is judged by the **rider's**
   eyes in water (not the horse's), skipped in a bubble column (section 5).
9. 5.15's dismount (item 6), `maxUpStep`, tracking and leash items agree with the
   bytecode.
