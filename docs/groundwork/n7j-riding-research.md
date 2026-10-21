# N7j groundwork: riding the camel, the nautilus and the happy ghast

Read from the 26.3 `javap -c` dumps on 2026-10-21, for part R of N7j
([the plan](n7j-plan.md), sections 3 item 10 and 4.2). Short names: `Camel` =
`n7-javap/…animal.camel.Camel.txt`, `Husk` = `…animal.camel.CamelHusk.txt`, `Horse` =
`…animal.equine.AbstractHorse.txt`, `Ghast` = `…animal.happyghast.HappyGhast.txt`,
`Naut` = `…animal.nautilus.AbstractNautilus.txt`, `Nautilus` and `ZNaut` its two
subclasses; `Entity`, `LivingEntity`, `Mob`, `Player`, `Leashable`, `SynchedEntityData`
and `SGPLI` (`ServerGamePacketListenerImpl`) are the `n7-javap` files; `ServerPlayer`,
`PlayerList`, `CollisionGetter` and `EntityGetter` the `n5-javap` ones. Numbers are file
lines, "@n" a bytecode offset. **Inferred** marks what was not read; **Client** what runs
only on the client, which is not in the jar. "B 7.7.5" is `n7-brief.md` section 7.7.5,
"R1" to "R9" the sections of [the N7h note](n7h-riding-research.md), whose rules this note
does not repeat. Clustine's files are under `crates/clustine-sim/src/` unless named.

**What of N5 and N7h applies unchanged** (R, "What of N5 applies unchanged"; R3, R5,
R7): `startRiding`'s checks, `addPassenger`'s order (a player is put first where the first
is not a player, Entity 7088–7135 — so a player who mounts a camel husk ridden by a husk
takes the first seat), the passenger tick, `rideTick`, the authority rules, the sneak
dismount (`Player.rideTick`, Player 753–770: `wantsToStopRiding` is the shift key, for
every vehicle, flying ones too), the water rule by the rider's eyes, `ServerEntity`'s
rider branch, the effects sent at mount. The three are living roots driven by a player's
client as the pig and the horse are (R3, R9.3). What is new is below.

## 1. Mounting

### 1.1 The camel and the camel husk

`Camel.mobInteract` (Camel 768–832), in order: sneaking and not a baby → its saddle menu
(`openCustomInventoryScreen`, 1554–1566: **no test of who rides it**, unlike the horse's
and the nautilus's; so anyone may open a ridden camel's saddle slot, and the inventory
key opens it for either rider, SGPLI 5213– @212–242 takes `getVehicle()`, not the
controlled one); the item's `interactLivingEntity` (a saddle, a lead, a name tag) if it
consumes; food (cactus; the husk's rabbit foot) → `fedFood`; **fewer than two passengers
and not a baby → `doPlayerRide`, `CONSUME`** (Horse 1066–1090: stop eating and rearing,
the player's yaw and pitch set to the camel's, `startRiding`; then `Horse.addPassenger`,
1782–1795, snaps the rider to the camel's view); a calf and a golden dandelion in either
hand → the horse's; else **`FAIL`**, not `PASS`. Unlike the pig's, no saddle is needed to
get on (only to steer). `Husk.interact` makes it persistent first (B 7.7.7).

`canAddPassenger` is `passengers.size() <= 2` (Camel 1310–1321): room for a **third**,
which no click gives (the click tests `< 2`) but a forced or a mob's `startRiding` could.

### 1.2 The happy ghast

`Ghast.mobInteract` (482–525): a ghastling → `Animal`'s (snowballs). An adult: a held
item's `interactLivingEntity` first if it consumes (a harness put on, a lead, a name tag);
then **wearing a harness** (`isWearingBodyArmor`) **and not sneaking** → `doPlayerRide`
(527–537: `startRiding` on the server, result ignored), `SUCCESS` even when it is full;
else `Animal`'s. A snowball in hand does not stop the ride (it consumes nothing on an
adult). `canAddPassenger` is `size < 4` (592–602): **four seats**.

`addPassenger` (539–568): with no riders before, `playSound(HARNESS_GOGGLES_DOWN)` (volume
1, pitch 1: `Entity.playSound(SoundEvent)`, Entity 3856–3866, no draw); then, on the
server, if `scanPlayerAboveGhast()` is false the still timeout is set to 0, else lowered to
10 if above. `removePassenger` (570–590): the timeout set to 10 (still for ten ticks
after anyone gets off, a logout among them), and with nobody left `clearHome()` and
`HARNESS_GOGGLES_UP`.

### 1.3 The nautilus and the zombie nautilus

`Naut.mobInteract` (889–987, B 7.12.4, checked): a baby → `TamableAnimal`'s; tame and
sneaking → its menu (1149–1170: only unridden or ridden by that player, and tame);
taming and feeding items; the item's own use (saddle, armour); then **tame, not
sneaking, not food → `doPlayerRide`, `SUCCESS`** (457–472: `startRiding`, then
`clearHome()` if that left it with no passenger, i.e. when the mount was refused). No
saddle is needed to get on. `canAddPassenger` = `!isVehicle()` (253–261): one seat.

### 1.4 The seats

- **Camel** (`getPassengerAttachmentPoint`, Camel 1041–1104; B 7.7.5's formula checked
  against 1115–1272, right): `z = 0.5`; only **when it carries more than one**, the
  second gets `z = −0.7` and an `Animal` passenger `+0.2` (inside that branch: a lone
  animal stays at 0.5); `y = getBodyAnchorAnimationYOffset(first, 0, dims, scale)` with
  `dims` the pose's (sitting 0.945 high), 0.01 if removed; `Vec3(0, y, z·scale).yRot(
  −yRot·π/180)`. Standing adult: y = 2.375 − 0.375 = 2.0, the player's feet at camel
  y + 1.4 (the player's vehicle point 0.6, R6), 0.5 ahead or 0.7 behind; sitting and
  settled: 0.945 − 0.375 + 0.2 ≈ 0.77; between, by the pose time, which is **the game
  time** (`gameTime − |LAST_POSE_CHANGE_TICK|`, B 7.0.3 finding 2). `Horse.positionRider`
  (2317–2334) then sets a living rider's body yaw to the camel's.
- **Happy ghast**: no override of the attachment point or of `positionRider` (Ghast's
  method list): the type's four `PASSENGER` points, turned by its yaw, in seat order:
  (0, 4, 1.7), (−1.7, 4, 0), (0, 4, −1.7), (1.7, 4, 0) (extracted, `generated/
  entity_types.rs`, `happy_ghast`): front, left, back, right, feet 3.4 above its own.
- **Nautilus**: the type's (0, 1.1375, 0); a baby's would be (0, 0.5, 0) scaled, but a
  baby takes no saddle and a wild one no rider.

### 1.5 Who controls

- **Camel**: `Horse.getControllingPassenger` (2420–2438): saddled and the first passenger a
  `Player` → that player; else `Mob`'s (Mob 306–326): the first passenger if a `Mob` that
  `canControlVehicle()` and the camel not `NoAi`. A camel is always tame (Camel
  1549–1553), so a saddle is all. **Camel husk**: the same; a husk in the first seat
  controls it (N9's riders), and then drives it by **the camel husk's own move control
  and navigation**, which `Mob.getMoveControl`/`getNavigation` hand the rider of a mob
  (Mob 260–300). A player who gets on goes first (Entity 7088–) and takes control if it
  is saddled; `isMobControlled` (first passenger a `Mob`) is then false.
- **Happy ghast** (604–625): the first passenger, if it **wears a harness, is not on its
  still timeout** and is a player; else `Animal`'s. So a non-riding player standing on
  top takes control from the rider (section 4).
- **Nautilus** (263–281): the first passenger if saddled and a player; else
  `TamableAnimal`'s (the zombie's drowned rider, N9).

**Clustine.** No click mounts any of them: the camel's ride is a `Pass` seam
(`mobs/types/camel.rs` 482–511, which also returns `Pass` where the game returns `FAIL`),
the nautilus's `ride` does nothing (`nautilus.rs` 496–498), the ghast's "part R's: nobody
gets on yet" (`happy_ghast.rs` 929–). The hooks are there as plain functions:
`camel::riding::{click_mounts, can_add_passenger, steered_by_rider, seat}` (576–813),
`happy_ghast::riding::{can_add_passenger, steered, seats, passenger_added,
passenger_removed}` (680–794), `nautilus::steers` (542). The region's side is the
pig's and the horse's: `Did::Mount`/`Did::Ride` → `mount_player` (`region/animals.rs`
244–274; `region/riders.rs` 192–278), and the gaps are:
- `can_add_passenger` gives any mob one seat (`region/riding.rs` 484–495): the camel's
  second (and third) and the ghast's second to fourth are refused;
- `passenger_point(vehicle, index)` (`riding.rs` 1190–1233) takes the type's points (right
  for the ghast and the nautilus) but not the camel's: it has no game time and no count,
  and the camel's own `seat` (camel.rs 774–801) is not called; the rider's body yaw is
  not set (the horse's is not either, `seat_player`, riders.rs 280–314);
- `MobType::steered_by` (`mobs/types.rs` 259) is not implemented for the five, so
  `Region::ridden` (`region/mobs.rs` 465–490) gives `Ridden::Not` and `drives_root`
  (`region/boats.rs` 468–483) false: every move the driver's client sends is answered as
  a passenger's ("not controlling", the vehicle put back);
- the ghast's `addPassenger`/`removePassenger` hooks and the nautilus's `clearHome` are
  not called from `mount_player`/`dismount_player`.

## 2. The server's tick of a ridden one

### 2.1 Who simulates, and what the server knows of the rider's keys

As the pig (R3): no type here overrides `getMoveSimulationType`, so with a player in
control `isClientAuthoritative()` (Entity 10354–) is true and **the controller's client
simulates the move and sends `move_vehicle`** (**Client**); the server's
`canSimulateMovement()` and `isEffectiveAi()` are false, so `LivingEntity.aiStep`
(8455–8500, @551–610) runs no `serverAiStep` and, the controller being a `Player`,
`travelRidden` (7051–7077): `getRiddenInput`, `tickRidden`, motion set to zero.

**The server never has a rider's keys.** `ServerPlayer`'s `xxa`/`zza` are written only by
`LivingEntity.aiStep` (zeroed while immobile, 8326–8329) and `applyInput` (× 0.98,
8611–8625); nothing in `ServerPlayer` or `Player` sets them, and `jumping` is set only by
mobs' jump controls (`setJumping` callers: `JumpControl`, the fox, the rabbit).
`handlePlayerInput` (SGPLI 604–651) keeps the `Input` (`setLastClientInput`) and the
shift key only, and `getLastClientInput` is read only by minecarts. So on the server
`getRiddenInput` is always the zero-key one and every rule of `tickRidden` that reads
`zza` or `isJumping()` never fires there. This matters for one rule (2.2).

### 2.2 The camel

- `Camel.tickRidden` (505–525): `Horse.tickRidden` (1711–1767: `setRot(rotation.y,
  rotation.x)` from `getRiddenRotation`, `yRotO = yBodyRot = yHeadRot = yRot`; its jump
  branch only `isLocalInstanceAuthoritative`, @43, **Client**), then **if `player.zza > 0`,
  sitting and not in transition → `standUp()`**. By 2.1 that `standUp` runs on the
  rider's client only. **Inferred** from the code: mounted on a sitting camel and pressing
  forward, the rider's client stands its own camel up and rides off; the server's stays
  `SITTING` (its pose, `LAST_POSE_CHANGE_TICK` and 0.945-high box) until it is hurt, goes
  into water or its brain runs again after the ride, and other clients see a sitting
  camel move. Worth a probe (section 11) before Clustine copies or mends it.
- `getRiddenRotation` (559–576): a refusing camel's own `(xRot, yRot)`; else the horse's
  `(rider xRot · 0.5, rider yRot)` (Horse 1769–1781). `getRiddenInput` (577–589): zero
  while refusing, else the horse's (1797–1839). `getRiddenSpeed` (539–558) and `travel`
  (478–504) are the client's.
- `Camel.tick` (293–359) runs on both sides whoever drives (B 7.7.3, checked): the dash
  ends when `dashCooldown < 50` and on the ground, in a liquid or a passenger; the
  cooldown counts down; at 0 `level.playSound(null, blockPosition(), DASH_READY,
  NEUTRAL, 1, 1)`; a refusing camel's head is clamped; sitting in water stands it up at
  once.

### 2.3 The happy ghast

- `tickRidden` (710–751): `Animal`'s, then `yaw += wrapDegrees(riderYaw − yaw) · 0.08`,
  `setRot(yaw, rider xRot · 0.5)`, `yRotO = yBodyRot = yHeadRot = yaw` (B 7.11.4,
  checked). On the server it runs **after** `handleMoveVehicle` has set the rotation the
  client sent (packets come before the tick), which the client's own `tickRidden` had
  already turned by 0.08: so the server turns it a second step, and others are shown the
  ghast one step nearer the rider's yaw than its driver's client has it. Clustine should
  keep that (it is what the game sends), not mend it.
- `getRiddenInput` (627–695; B 7.11.4 checked): `(xxa, s, c) × 3.9·flyingSpeed`, the
  vertical `−sin(pitch)` along the look while a forward or back key is held (half and
  reversed going back), **+0.5 while jumping**; there is no descend key: sneaking gets
  off (Player 753–770). `travel` (276–293) flies with `travelFlying`. All **Client**.
- Not on still timeout and driven, its AI does not run; on still timeout it is not driven
  (1.5) and the server's AI runs: the ghast's move control stops it in place
  (`shouldBeStopped`, B 7.11.2) and the look control snaps its yaw to a quarter turn,
  the riders aboard.
- `aiStep` (849–863): `setRequiresPrecisePosition(isOnStillTimeout())` on the server each
  tick, then `Animal`'s, then `continuousHeal`.

### 2.4 The nautilus

- `tickRidden` (353–413): `TamableAnimal`'s; `yaw += wrapDegrees(riderYaw − yaw) · 0.5`,
  `setRot(yaw, rider xRot · 0.5)`, head, body and old yaw to it; **then, only
  `isLocalInstanceAuthoritative()` (@71)**, a pending jump executes and is cleared. The
  server applies the half-turn a second time after the vehicle packet, as for the ghast.
- `getRiddenInput` (283–338): strafe `xxa`, and with a key `zza ≠ 0` the look's
  `(−sin pitch, cos pitch)`, halved and reversed going back; no jump term.
  `getRiddenSpeed` (437–455): `0.0325 · speed` in water, `0.02 · speed` out.
  `travelInWater` (415–435): `moveRelative(speed, input); move; motion × 0.9`, no
  gravity. All **Client**.
- `Naut.tick` (682–725) on both sides: on the server `applyEffects` (Breath of the
  Nautilus to a player first passenger, N9's); the dash ends when `dashCooldown < 35`
  (no ground test, unlike the camel's); the cooldown counts down and at 0 `makeSound(
  dash ready)` (two draws, 2.6); bubbles in water (draws, nothing sent).

### 2.5 Their moves: `move_vehicle` and `handleMoveVehicle`

For all three the driver's client sends `move_vehicle` each tick (**Client**), and
`handleMoveVehicle` (SGPLI 669–935) checks it as a boat's (n5-brief 2.3) and a pig's
(R3): taken only when the root's controller is the player and it is `lastVehicle`
(@80–116); otherwise, if 20 ticks have passed since the last reset or the move is more
than a block off, the vehicle is sent back (`ClientboundMoveVehiclePacket`, @845–932).
What differs:
- the step height is `max(STEP_HEIGHT, 1.0)` with a player in control
  (`LivingEntity.maxUpStep`, R3): the camel's 1.5, the ghast's and nautilus's 1.0;
- **the floating kick spares a flying vehicle**: `clientVehicleIsFloating` needs
  `!vehicle.isFlyingVehicle()` (@782–787), and `Ghast.isFlyingVehicle` (1235–1243) is
  true for an adult. The nautilus (`Entity.isFlyingVehicle` false, 11377–11381) is
  counted, but water is not air, so `noBlocksAround` holds only out of water (80 ticks,
  its gravity 0.08, **Inferred** default attribute);
- the accepted move's `doCheckFallDamage`: the ghast's `checkFallDamage` is empty
  (267–270), so **no fall** is gathered or landed; the camel's fall is the horse's (R9.7:
  safe 6, × 0.5, no damage to it then none to its riders); the nautilus's a living
  body's;
- the replay (`Entity.move`) meets entities by `canCollideWith` (Entity 6765–6778): a
  ridden ghast meets still ghasts and ghasts carrying riders (section 4), boats, shulkers.

**Clustine.** `move_vehicle_tested` and `check_living_root_move` (`boats.rs` 1548–1660,
1715–1767) take a living root with its own attributes, the raised step, the client's
ground and the fall passed to the landing (`living_root_landed`); `Ridden::Driven` gives
no AI and zero motion and calls `tick_ridden` (`mobs/tick.rs` 278–293). Gaps:
- **the floating kick for the ghast**: `check_move` counts floating by `root.gravity > 0`
  and `no_blocks_around` (`physics/vehicle_move.rs` 307–311) and `tick_drivers` puts a
  living root at 80 ticks (`boats.rs` 1856–1888); a ghast's rider would be kicked with
  "Flying is not enabled" after four seconds in the air;
- the ghast's fall: `check_move` gathers a living root's fall and `living_root_landed`
  lands it; the ghast gathers none (it is also `#fall_damage_immune`, so only a landing
  sound or a passed fall could show);
- `tick_ridden` hooks for the five (the camel's exists, `camel::riding::tick_ridden`,
  with a `forward` the region must pass as 0 to be the game's; the ghast's and the
  nautilus's are `ridden_yaw` and `ridden_rotation`);
- the camel's 1.5 step is read from its attributes already (`living_attributes`); its
  sitting box must be its own (`dimensions_of`, camel.rs 339) in the replay.

## 3. Dashes and jumps

**What the client sends** (**Client**): holding jump charges the bar of a
`PlayerRideableJumping` vehicle whose `canJump()` holds (`getJumpCooldown()` greys it out:
the camel's and nautilus's `dashCooldown`); on release the client calls `onPlayerJump(
power)` itself, then sends `player_command START_RIDING_JUMP` with the power (0 to 100),
and executes the jump in its own `tickRidden`. `onPlayerJump` has **no caller in the
jar** (the classes that call it are only the three that define it): the server never
runs it, nor `executeRidersJump`, which only the `isLocalInstanceAuthoritative` branch of
`tickRidden` calls (Horse 1711– @43, Naut 353– @71).

**What the server does** (`handlePlayerCommand`, SGPLI 5213–5276): for `START_RIDING_JUMP`
(@135–180), the **controlled** vehicle (`getControlledVehicle`) if a
`PlayerRideableJumping` with `canJump()` and `data > 0` → `handleStartJump(data)`;
`STOP_RIDING_JUMP` (@183–209) → `handleStopJump()`, nothing for all three. **No cooldown
is tested on the server**: a client that sends the command again gets the sound again.
The happy ghast is not `PlayerRideableJumping`: the command does nothing.

- **Camel**: `canJump` = `!refuseToMove() && saddled` (590–602, the horse's 2223–2228
  tests the saddle): sitting refuses it. `handleStartJump` (693–706): `makeSound(
  CAMEL_DASH)` (husk `CAMEL_HUSK_DASH`), `gameEvent(ENTITY_ACTION)`, `setDashing(true)`.
  `makeSound` (LivingEntity 3570–3581) plays at `getSoundVolume()` = **0.8**
  (Horse 793–796) and `getVoicePitch()` = `(r.nextFloat() − r.nextFloat()) · 0.2 + 1`
  (5935–5960): **two draws of the camel's random**, then the level's sound seed (Level
  775–777), as for every sound. **`setDashing` starts the server's cooldown**:
  `SynchedEntityData.set` calls `onSyncedDataUpdated` wherever the value changes
  (SynchedEntityData 101–135), on the server too, and `Camel.onSyncedDataUpdated`
  (1526–1548) sets `dashCooldown = 55` if it was 0 (not on the first tick). So on the
  server the dash shows from the command, its cooldown runs 55 ticks from it, `DASH`
  clears once the cooldown is under 50 and the camel is on the ground, in a liquid or a
  passenger (at least 6 ticks), and the ready sound plays 55 ticks after the command, to
  every player near (the rider among them); the ready sound draws nothing of the camel's.
  The client's own dash (`executeRidersJump`, 625–672: `look.xz.normalize() · 22.2222·s ·
  speed · blockSpeed` across and `1.4285·s · jumpPower` up, about 2 blocks a tick at a full
  charge; cooldown 55, `DASH`, `needsSync`) comes as its moves.
- **Nautilus**: `canJump` = saddled (727–731). `handleStartJump` (817–829): `makeSound(
  getDashSound())` (`Nautilus` 153–: `NAUTILUS_DASH` under water by its eyes,
  `_ON_LAND` else; the zombie's likewise), volume 1, **two draws**; `ENTITY_ACTION`;
  `setDashing(true)` → cooldown 40 if it was 0 (837–858). The client's dash
  (`executeRidersJump`, 782–815): along the **rider's** look, `(1.2 in water, 0.5 out) ·
  s · speed · blockSpeed`, cooldown 40. The ready sound at 0 is `makeSound`: two draws.
- **Happy ghast**: no dash; jump is its rise (2.3).

**Clustine.** `riding_jump` (`riders.rs` 97–147) takes `RidingJump { power }` only for
the horse family (`equine::is_equine`, which leaves the camel out, `equine.rs` 235–252).
The nautilus's hooks are right (`nautilus.rs` 585–640: `handle_start_jump` begins the 40
where none runs). **The camel's `handle_start_jump` (camel.rs 659–662) sets `dashing`
without the cooldown**, so its `after_tick` (886–) ends the dash at once on the ground
and never sounds it ready; it must begin `dash_until = lived + 55` where it is 0, as the
game's `onSyncedDataUpdated` does. The sound's two draws, the `ENTITY_ACTION` vibration
and the sound event are the caller's for both. `execute_riders_jump` of both is the
client's and is called by nothing on the server.

## 4. The happy ghast's staying still

**The timeout** (B 7.11.4, checked against 65–101, 798–847, 1106–1186): `tick()` on the
server, after `Animal.tick` (so after this tick's `aiStep`): `leashHolderTime`; the
timeout minus 1 while above 0 and `tickCount > 60`, resynced; then **if a player is
above** (`scanPlayerAboveGhast`: every player of the level, not a spectator, whose root
vehicle is not a happy ghast, with the **root vehicle's position** in the slab from the
top less 1e−5 up half the ghast's height, a block beyond each side) → 10. While a player
stands there it stays 10. `setServerStillTimeout` from ≤ 0 to > 0 sends
`ClientboundEntityPositionSyncPacket.of(this)` to every tracker, its riders among them;
`STAYS_STILL` (index 19) is `timeout > 0`. `isOnStillTimeout` = `STAYS_STILL ||
timeout > 0`. Riders of the ghast never hold it (their root is a ghast); a player in a
boat on its top does, by the boat's position. Mounting with nobody on top clears it
(`addPassenger`); getting off sets 10 (`removePassenger`), so a ghast stops for ten ticks
when any rider leaves, the remaining driver without control meanwhile.

**Collision** (`canBeCollidedWith(other)`, 1196–1233): false for a ghastling or a dead
one; **on the client** true for a player at or above its top (**Client**: anyone may land
on any adult ghast in their own client); true where it carries riders and `other` is a
happy ghast; else `isOnStillTimeout()`. On the server this feeds
`Entity.canCollideWith` (6765–6778, not of one's own unit) in every `Entity.move` of
another entity (`getEntityCollisions`, EntityGetter 83–): mobs, items, boats, carts and a
replayed vehicle stop on a still ghast; two ridden ghasts meet.

**Players standing on it, and the server's player check.** `handleMovePlayer` replays the
move by `ServerPlayer.move(PLAYER, …)` (SGPLI 2913–, @557), which meets entities as above,
and then judges with `noCollision(player, box)` and `isEntityCollidingWithAnythingNew`
(@743–783, 3740–3767), both of which count entity collisions (`noCollision` →
`noEntityCollision`, CollisionGetter 62–92; `getPreMoveCollisions`, 193–217). A player on
a still ghast is therefore replayed onto its top, and one pushing into its side is
stopped, as their client does. Nothing in the game moves a player who stands on a moving
ghast: within a tick the scan stops it.

Clustine takes a player's move as their client says (`region.rs` 2042–2077: no replay,
no "moved wrongly", every player creative), so **the player's check needs nothing**; the
client lands on the ghast by its own rule. What does need the ghast's solidity is every
other mover: `physics/entities.rs` judges solidity by kind alone (`is_solid` = boats,
115–117; `Mover::can_collide_with`, 141–154; `colliders_in`, `region/colliders.rs` 79–),
which needs a per-record answer from the ghast's state (`happy_ghast::riding::
can_be_collided_with`, 771–783, which the plan's N7j1 hook `can_be_collided_with` is for)
and the mover's kind (another ghast). The scan is built (`happy_ghast.rs` 211–250,
`after_tick` 884–898) over `MobWorld::players()`, the region's players and the faces;
it walks the vehicle chain and refuses a happy ghast anywhere in it, where the game tests
only the root (the same for every real case). The position packet and the precise
positions while still are the edge's by `stays_still` (named in `happy_ghast.rs`); the
edge already sends a whole position for a report flagged `Tracked::PRECISE`
(`services/edge/src/tracking.rs` 1118–), which the ghast's report must set while still.

**The leash and its harness, as far as riding needs them.** The harness is the body
slot; riding needs one, and so does control. Shears take no saddle or harness off a mob
that carries anyone (`Mob.canShearEquipment` = `!isVehicle()`, Mob 1222–1230). A ridden
ghast may be leashed; its leash pulls by `getKnownMovement` (Leashable 500–560), which is
the controlling player's while a player drives (Entity 11467–11486). Clustine's
`driver_of` in `region/leashes.rs` (419–432) knows only boats, so a driven mob's pull and
a passenger's known movement read the mob's zero motion. `checkRestriction` keeps no home
while ridden or leashed (B 7.11.5); `onElasticLeashPull` makes the move control wait.

## 5. Getting off

- **Camel**: `Horse.getDismountLocationForPassenger` (2544–2600): the side of the rider's
  main arm first (`getCollisionHorizontalEscapeVector(width, riderWidth, yRot ± 90)`,
  then `getDismountLocationInDirection`, 2440–2543, each pose, floors within 0.75), then
  the other side, then **the camel's own position** (not on top). Clustine has it as
  `equine::riding::dismount_directions`, which `camel::riding::dismount_directions`
  passes the camel's width (camel.rs 803–812); `dismount_location` (`riding.rs` 901–)
  must take the camel into its horse branch.
- **Happy ghast** (1245–1257): `(x, box maxY, z)`, on its top at the middle, always:
  `happy_ghast::riding::dismount_at` (790–793).
- **Nautilus**: none of its own, so `Animal`'s (the pig's, R5).
- **Water** (R5, the rider's eyes, not in a bubble column): `#dismounts_underwater` holds
  the **camel** and the **happy ghast** but **not the camel husk, the nautilus or the
  zombie nautilus** (`generated/tags.rs` 2762–2766: 19, 59, but not 20, 89, 156). So a
  ghast flown into water drops its riders whose eyes go under, a camel husk keeps them,
  and the nautilus's purpose is kept. Clustine's `rider_under_water` (`riders.rs`
  638–666) reads the tag, and is right for all five as it is.
- Sneaking: every one (2.1). A dead or removed vehicle: N5's, as the pig's (R5).

## 6. What clients are shown

As the pig and the horse (R7): `set_passengers` in seat order (the edge's
`passengers_packet` and `effective_passengers`, `services/edge/src/fanout.rs`
6447–6510, already list any number), the vehicle's moves and rotation from its tracker,
the rider's head only, the tracking range of a ridden vehicle its riders'. Then:

| | Synced data | Equipment | Sounds (server) | Game events |
|---|---|---|---|---|
| Camel | flags 18 (the family's), `DASH` 19, `LAST_POSE_CHANGE_TICK` 20 (long), pose 6 | saddle (`SADDLE`) | dash (0.8, voice pitch), dash ready (block centre, 1, 1, to all), sit and stand (`makeSound`) | `entity_action` (dash, sit, stand), `entity_mount`/`_dismount` |
| Husk | the same | saddle | its own of each (B 7.7.7) | the same |
| Ghast | `IS_LEASH_HOLDER` 18, `STAYS_STILL` 19 | harness (`BODY`) | goggles down at the first rider, up at the last (1, 1) | mount, dismount |
| Nautilus | `DASH` 20 (zombie: variant 21) | saddle, body armour | dash and dash ready by medium (voice pitch) | `entity_action` (dash), mount, dismount |

The ghast also sends `entity_position_sync` as it begins to stay still, and exact
positions while still (2.3; `ServerEntity`'s precise branch, N5). No entity event belongs
to riding (the nautilus's 6 and 7 are taming's). The ghast's ambient sound comes six
times as seldom while ridden (B 7.11.1; named as missing in `happy_ghast.rs`). The
nautilus's Breath of the Nautilus is N9's (the plan's 1.2). The second rider of a camel
and the ghast's other three riders are shown seated by `set_passengers` alone; their
clients send rotation-only moves (**Client**), which the region already takes as a
passenger's.

## 7. Across a line

What a unit needs of N5's crossing (`riding.rs` `relocate` 367–414, `riders.rs`
`riders_follow` 682–728, `rider_arrived` 730–801):

- **Nothing in the hand-over assumes one player seat**: `riders_follow` marks every
  player seat of every member pending and moves each seated rider's holding chunk with
  the unit; each rider is handed on by their own transfer and seated by `rider_arrived`
  in either order; a boat already carries two. A camel with two players, a ghast with
  four, a nautilus with one cross as a boat does, the unit first, its riders after it.
- **One driver**, the first seat, is assumed (`drives`, `drives_root`, `Ridden`), and is
  the game's for all three. It changes hands when the first rider gets off, logs out, or
  lets their pending seat lapse after 100 ticks: the second becomes driver, and their
  client starts sending moves; the check is seeded at the end of the tick by
  `tick_drivers` as the game's `lastVehicle` is (n5-brief 2.2), so the first such move
  may be ignored. With the first seat pending and a second seated, the mob is
  `Ridden::Pending` (kept still) until the first comes or lapses: the game's would be
  driven by its first passenger, who is the pending player.
- **Control that turns with the ghast's state**: a ghast passes between driven and the
  server's with its still timeout. A pending driver's kept moves are judged at their
  seating by who controls then (named in `keep_vehicle_move`, riders.rs 842–853); a
  ghast that became still meanwhile refuses them, which is the game's answer then.
- **Where riders are in two regions**: (a) while the unit crosses, a pending rider is
  still in the old region, for one to three ticks, or up to 100; (b) a rider's **seat**
  may be in another region's land while their holding chunk is the unit's: the camel's
  seats are 0.5 ahead and 0.7 behind, the ghast's 1.7 out from its middle, so a ghast
  whose middle is 1.7 from a line has a rider standing beyond it; the water rule, the
  eye test and the dismount read such a seat's blocks through the copies, which reach a
  chunk beyond; (c) a player standing on a ghast across a line is a face to the ghast's
  region, and holds it still a tick after their region publishes the face; (d) a mount
  across a line is not made (`region/animals.rs` 241–245: "one across a line does not get
  on, as on a boat"): with a ghast 4 wide, a click from the next region over is likely,
  and a player would notice it.
- **A dismount into another region**: the camel's side (up to about 1.15 blocks from
  its middle) or the ghast's top may be in the next region's land; the player is put
  there and handed on by their position, as from a boat (N5).
- **Moving fast**: a dashing camel crosses about 2 blocks a tick and may cross a line the
  tick after it is on it; the dash's state is the record's (`CamelState`, `dash_until` a
  moment of its own `lived`; the nautilus's `dash_ready_at`), so it travels; the camel's
  seat and the riders' height read the game time, which must be the world's on both
  sides (B 7.0.3 finding 2; the plan's 3 item 11 names a second's error for a sitting
  camel's pose read across a line, which here would move riders by up to a sitting
  animation's step).
- **The camel husk's mob riders** (N9) are members of the unit and cross with it.

## 8. Logging out on these mounts

`PlayerList.remove` (PlayerList 575–680; R8): only where the root
`hasExactlyOnePlayerPassenger()` — `countPlayerPassengers` counts players among **all
indirect passengers** (Entity 10255–10274) — is the unit taken with the player
(`saveParentVehicle`, every member `UNLOADED_WITH_PLAYER`). Otherwise `unRide()`:
`stopRiding`, the dismount computed, and the unit stays.

- **Two players on a camel, two to four on a ghast**: the leaver is put off where the
  vehicle lets them off (the camel's side, the ghast's top), the unit stays with the
  others, the next seat becomes first and drives; the ghast's `removePassenger` stops it
  for ten ticks.
- **One player and mob riders** (a camel husk with a husk behind its player, N9): one
  player, so the whole unit leaves with them, the mobs included, and comes back with them.
- **One player alone**: as the pig.

Clustine's `take_unit_leaving` (`riders.rs` 380–450) counts player seats over every member,
seated or pending, and takes the unit only when the one is the leaver's, seated: the
game's rule for two and four players and for mob riders. Not the game's: a lone player
whose unit holds a second player's **pending** seat is put off (the game would count only
players aboard); named in the logout design (section 8 row 18).

## 9. The brief checked

1. **B 7.12.5 is wrong** that "on the server, if a jump is pending … `executeRidersJump`":
   the branch is `isLocalInstanceAuthoritative()` (Naut 353–413 @71), the driver's client;
   the server never has a pending scale (`onPlayerJump` has no caller in the jar).
2. **B 7.7.5 is wrong** that `executeRidersJump` comes "from the horse's travel": it comes
   from `Horse.tickRidden` (1711–1767 @43), the client's alone; and its "On the client, a
   `DASH` update … sets `dashCooldown = 55`" holds **on the server too**
   (SynchedEntityData 101–135): the server's cooldown begins at `handleStartJump`. The same
   for the nautilus's 40 (B 7.12.5 says "after the first tick", rightly, without the side).
3. **B 7.7.4's "A rider pressing forward stands it up"** runs only on the client (2.1, 2.2):
   the server's rider has no `zza`.
4. **B 7.11.4's goggles sounds** are at volume 1, pitch 1 (`Entity.playSound(SoundEvent)`,
   Entity 3856–3866), not the entity's volume and pitch; no draw.
5. **B 7.7.5's seats**: the `Animal` rider's `+0.2` applies only with more than one
   passenger (Camel 1041–1104 @58–96).
6. **Silent**: the flying vehicle's exemption from the floating kick (SGPLI @782–787); the
   ghast's empty `checkFallDamage`; the second turn of the ghast's and the nautilus's
   yaw on the server (2.3, 2.4); the camel's saddle menu open to anyone (1.1); the camel's
   `FAIL` and the ghast's `SUCCESS` when full; that no saddle or harness is needed to get
   on a camel or a nautilus; the dash sounds' volume 0.8 (camel) and their two draws;
   that the server tests no dash cooldown before `handleStartJump`; `#dismounts_underwater`
   (camel and ghast in, the husk and the nautili out); that a player mounting a mob-ridden
   camel husk goes first and steers.
7. B 7.7.3's "the rider may hear [the dash ready] twice" is **Client** and unread; the
   server sends it to the rider as to everyone near.
8. The rest of 7.7.5, 7.11.4 and 7.12.5 agrees with the bytecode: the controllers, the
   inputs and rotations, `canAddPassenger` (the camel's `<= 2`), the still timeout and its
   grace, the scan's box, `canBeCollidedWith`, the dismount on top, the dash formulas.

## 10. What Clustine lacks, in the order a player would notice it

1. **Getting on**: the three clicks (camel and husk with fewer than two, `CONSUME`, else
   `FAIL`; ghast harnessed and not sneaking, `SUCCESS`; nautilus tame, not sneaking, not
   food, `SUCCESS`), `can_add_passenger` by type (camel `<= 2`, ghast `< 4`, nautilus 1),
   the camel's `doPlayerRide` rotation, the ghast's goggles and still timeout at
   `addPassenger`/`removePassenger`, the nautilus's `clearHome` (1.5).
2. **Steering**: `steered_by` for the five (saddled; harnessed and not still; saddled),
   so the driver's moves are taken and the AI stops; the three `tick_ridden` hooks (the
   camel's with `forward` 0) (2.2–2.4).
3. **The ghast's rider kicked for flying** after 80 ticks: `isFlyingVehicle` in the
   floating test, and no fall for the ghast (2.5).
4. **The seats**: the camel's two by pose and game time, re-seated every tick, the riders'
   body yaw; the ghast's four from its type (already right) (1.4).
5. **Dashes**: `RidingJump` for the camel and the nautilus (`canJump`, `handleStartJump`
   with its sound, two draws, `ENTITY_ACTION`, `DASH`), and the camel's cooldown begun at
   the command (3).
6. **Getting off**: the camel in the horse branch of `dismount_location`, the ghast on
   top, the nautilus as the pig (5).
7. **Standing on a still ghast**: its solidity per record for other entities' and
   vehicles' moves, the precise flag and the position packet as it stops (4).
8. **Menus**: the camel's saddle menu by sneaking or the inventory key (`open_mount_menu`
   takes only the horses of `equine::of`, named in camel.rs), the nautilus's (with its own
   rider test) (1.1, 1.3).
9. **Mounting across a line**, likelier with a ghast 4 wide (7 (d)).
10. **The leash of a driven mob**: `driver_of` in `leashes.rs` for mobs (4).
11. The ghast's ambient interval while ridden; Breath of the Nautilus (N9); the camel
    husk's mob riders and their steering by its controls (N9).

## 11. Worth probing against the official server

1. A rider who mounts a sitting camel and presses forward: what a second client is shown
   (the pose index 6 and index 20) and whether the server's camel ever stands (2.2).
2. A camel dash: the order and ticks of `DASH` true, `DASH` false and the ready sound to a
   watcher, from the command (3).
3. A ghast ridden by two, a third player landing on its top: the position sync packet,
   `STAYS_STILL`, the vehicle packets sent back to the driver (4).
4. A ghast held in the air by its rider for 100 ticks: no kick (2.5).
