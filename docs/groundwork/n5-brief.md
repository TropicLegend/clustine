# N5 porting brief: vehicles, riding and what a creative player places (26.3, protocol 777)

Read from `server-26.3.jar` with `javap -p -c -constants` (names are Mojang's), one file
per class, and from `target/datagen/26.3/generated/`. Every rule below is the bytecode's
unless marked **Inferred** (reasoning, to test) or **Client** (client code, not in the
server jar). Method names are the game's; "line" ranges, where given, are of the `javap`
output of that class ("Entity.txt" is `net.minecraft.world.entity.Entity`'s, and so on),
so a porter can go straight to them:

```
J=target/datagen/26.3/versions/26.3/server-26.3.jar
$HOME/.local/jdk/bin/javap -p -c -constants -cp $J net.minecraft.world.entity.Entity
```

The 72 classes read are `Entity` (and its nested classes), `ServerEntity`,
`ServerGamePacketListenerImpl`, `InteractionResult`, `VehicleEntity`, `DismountHelper`,
`ContainerEntity`, every minecart and both minecart behaviours, `AbstractBoat`, `Boat`,
`ChestBoat`, `Raft`, `ChestRaft`, `AbstractChestBoat`, `ArmorStand`, `BlockAttachedEntity`,
`HangingEntity`, `ItemFrame`, `GlowItemFrame`, `Painting`, `PaintingVariant(s)`,
`LeashFenceKnotEntity`, `Leashable` (and `LeashData`, `Wrench`), `Mannequin`, `Cushion`,
the rail blocks, `RailState`, `RailShape`, and the items `MinecartItem`, `BoatItem`,
`ArmorStandItem`, `HangingEntityItem`, `ItemFrameItem`, `LeadItem`. What else a rule
depends on is named in section 11 ("Not read"). `invokedynamic` targets (lambdas and
method references) are not shown by `javap -c`; where a rule depends on which method one
is, it is marked **Inferred**. Where SteelMC's 26.3 extraction is quoted (attachment
points, `LivingEntity`'s data ids, serializer ids), it is for orientation only and says
so.

Conventions throughout (as in `docs/groundwork/n4-brief.md`): `double` is `f64`, `float`
is `f32`; a `float` constant used in `double` arithmetic is widened (`f2d`) *after* any
float arithmetic it takes part in; no fused multiply-add (never call `mul_add`);
`Mth.floor(double)` is `(int)Math.floor(x)`, which Rust's `x.floor() as i32` matches;
Java `int` arithmetic wraps and `%` (also `frem` on floats) takes the sign of the
dividend, as Rust's `%` does; `Mth.sin`/`Mth.cos` are the game's table (the repo's
`clustine_noise::trig`).

Each of sections 1 to 7 opens with the findings of its own reading (x.0); section 0
gathers those that change what the plan and ADR-0028 assume, with pointers.

---------------------------------------------------------------------------------------

## 0. What the step should know before planning (findings)

**What 26.3 is**

1. **Minecarts move by `OldMinecartBehavior`.** `AbstractMinecart`'s constructor takes
   `NewMinecartBehavior` only when `level.enabledFeatures()` contains
   `FeatureFlags.MINECART_IMPROVEMENTS`, which only the experimental data pack
   `minecart_improvements` gives (`generated/data/minecraft/datapacks/`). A new world
   uses the old behaviour. The new one moves up to `max_minecart_speed`/20 blocks a tick
   along rails many blocks ahead and needs its own packet (`move_minecart_along_track`,
   56): **Inferred** recommendation, refuse or ignore the experiment in N5 (3.0 items 1,
   6; 3.9).
2. **Boats are still driven by the client.** A boat's controlling passenger is its first
   passenger if that is a `Player`, else none. While a player controls it the server
   zeroes its motion and turning every tick and neither floats, steers nor moves it; the
   client sends `move_vehicle` (serverbound 34: position, rotations and, new, `onGround`)
   and `paddle_boat` (35) every tick, and the server answers with `move_vehicle`
   (clientbound 58) only when it refuses a move. `AbstractBoat.setInput` is called by
   nothing on the server: the keys never reach it. A boat with no player in front (empty,
   or a mob in front) is the server's to simulate: floating, gravity, currents, bubble
   columns, its move and picking up entities; a boat just left starts from zero motion
   (2.0 item 1; 4.0 items 1, 2).
3. **Minecarts have no controlling passenger**: the server moves them. A rider's keys
   reach a cart only through `player_input` → `ServerPlayer.setLastClientInput` →
   `getLastClientMoveIntent()`, which the cart adds, unnormalised, × 0.001, only while it
   moves under 0.1 a tick (also cancelling an unpowered powered rail's brake)
   (2.0 item 1; 3.6 step 10).
4. **The use-entity packet is split.** `attack` (serverbound 1: the entity id) and
   `interact` (26: id, hand, the hit location, `usingSecondaryAction`); there is no
   `interactAt` and no handler object. `Entity.interact(player, hand, location)` takes the
   location (the armour stand's slot by height). `handleInteract` sets the player's sneak
   flag from the packet before anything else, so `canRide` and `isSecondaryUseActive`
   see the packet's flag, not the last `player_input` (the edge keeps sneaking from
   `PlayerInput` only today) (2.0 items 2, 3).
5. **`Cushion` is new in 26.3, stable, and belongs in N5.** A `BlockAttachedEntity` lying
   on a block (1.0 × 0.25), sixteen dyed items (`minecraft:<colour>_cushion`, stack 16,
   in no experimental pack), sat on by a right click (`startRiding`, one rider) and left
   by sneaking; one hit breaks it, a creative breaker gets nothing; it goes when nothing
   under it holds it (checked every 101 ticks). It never moves: the simplest vehicle.
   Its placing item's class is not read (5.0 item 1; 5.8).
6. **`Mannequin` is a player-shaped `LivingEntity`** (`extends Avatar`) with no item and
   no spawn egg (summoned only). It needs all of `LivingEntity`: **not N5**; keep one
   met in a world as an entity of unknown behaviour (its NBT kept) until N7 (5.0 item 2;
   5.9).
7. **An armour stand is a `LivingEntity`.** Its gravity, drag, fluids, fire, health,
   equipment (`SetEquipment`) and attributes (`UpdateAttributes` at pairing) are
   `LivingEntity`'s, which is not read. The plan must choose between porting
   `LivingEntity`'s movement now (N7 needs it anyway) and a reduced stand (5.0 item 3;
   5.5).
8. **Only boats are leashable among N5's entities** (`AbstractBoat implements
   Leashable`; minecarts, stands, mannequins, cushions and hanging entities are not).
   Any entity can hold a leash, so N5 meets a boat held by a player, tied to a fence (a
   knot), and held by another boat (sneak and use a boat while holding a leashed one).
   The pull is a damped spring with a torque on the yaw (the "wrench"), past 6 less both
   widths between box centres; the leash breaks beyond 12 with a lead dropped (7.0).

**What changes ADR-0028 section 5 ("Vehicles and riders")**

9. **The order of passengers is the state, not a seat number.** On the server
   `addPassenger` puts a player at index 0 when the first passenger is not a player, and
   appends anyone else; the boat steers by index 0 if it is a player, and seats by index
   (front 0.2, back -0.6 along the boat). A `PlayerTransfer`'s "seat" must be this order,
   and seating a pending player follows this rule (1.0 item 1; 4.6).
10. **A vehicle whose indirect passengers include exactly one player is not saved with
    its chunk** (`shouldBeSaved`): in the game it is saved with that player and leaves
    with them at logout (`UNLOADED_WITH_PLAYER`). With none or two or more players it is
    saved with its chunk, passengers nested in its `Passengers` list (a passenger is
    never saved on its own; its `Pos` is written as the vehicle's x and z with its own y).
    A decision for the plan: follow the game (a boat goes with the player who logs out in
    it) or name the difference (1.0 items 2, 3; 1.9).
11. **A rider is sent no position.** `ServerEntity` sends a passenger only `MoveEntity.Rot`
    on update ticks, rebases its position codec each update and sets `wasRiding`, so the
    first move after dismounting is a whole `entity_position_sync`; the client places
    riders. `SetPassengers` goes on any change of the list (by the vehicle's tracker) and
    at pairing both for the entity's own passengers and for its vehicle's. The edge's
    `Tracker` needs a "riding" flag (1.0 items 4, 5; 1.11).
12. **The check of a vehicle's move keeps state on the connection**, not the entity:
    `lastVehicle`, `vehicleFirstGood`, `vehicleLastGood`, `clientVehicleIsFloating`,
    `aboveGroundVehicleTickCount`, `vehiclePositionLastResetAt`. `handleMoveVehicle`
    takes the driver's branch only if the root `== lastVehicle`, which `tickPlayer` sets
    at the end of the player's tick: a `MoveVehicle` before that is answered with a
    correction. **Inferred**: the region that seats a crossing rider must seed these as
    `tickPlayer` would before it applies the kept `MoveVehicle`, or the boat jumps back
    (2.0 item 6; 2.2; 2.3).
13. **What a vehicle's move check is.** "Too quickly": the move from `vehicleFirstGood`,
    squared, exceeds the motion's squared length by more than 100 (about 10 blocks).
    "Moved wrongly": only the horizontal error counts (`if (dy > -0.5 || dy < 0.5) dy =
    0` zeroes every finite `dy`), wrong above 0.0625 squared, refused only if the old box
    was clear or the new place collides anew. An accepted move puts the vehicle at the
    **client's** position, not the collided one; a refused one sends it back with the
    client's rotations. There is no per-tick limit on `MoveVehicle` packets. A vehicle
    held in the air by its client kicks its driver after `ceil(80·max(0.08/g, 1))` = 160
    ticks unless the server allows flight (2.0 items 4, 7; 2.3).
14. **A riding player's own move packets move nothing**: only their rotation is taken
    (2.5). The player's position is the vehicle's `positionRider`.
15. **Vehicles carry vehicles.** A moving empty rideable minecart (0.1 a tick or more)
    makes any pushable entity it touches that is not a player, an iron golem or a cart
    ride it: a **boat** among them, so a cart can be the root of a boat. A boat with no
    player in front picks up `LivingEntity`s narrower than itself not in
    `#minecraft:cannot_be_pushed_onto_boats`: in N5 a mannequin (**Inferred**), never an
    armour stand (not pushable) (3.0 item 3; 4.0 item 10).
16. **`setLeashedTo` dismounts the leashee**: a leashed entity cannot ride (7.5).

**What the repo's physics lacks**

17. **Entity colliders.** A boat is solid to every entity (`canBeCollidedWith` true), and
    boats and carts collide with every entity that is a collider or pushable
    (`canVehicleCollide`). Items and falling blocks land on boats, a drifting boat or a
    rolling cart stops against a player's box (the client's, ADR-0005), a player stands
    on a boat. N4's collision has block shapes only (3.0 item 4; 4.0 item 4).
    Boats and carts also set `blocksBuilding` (no block placed into their box,
    **Inferred**).
18. **Ticks that skip `baseTick`.** `AbstractMinecart.tick` never calls it (no fire
    countdown, no freezing); hanging entities, knots and cushions never call
    `Entity.tick` at all (no gravity, no fluids). Boats call it (fluid pushing, the leash)
    (3.0 item 2; 6.0 item 2).
19. **Block effects applied twice a tick**: a boat calls `applyEffectsFromBlocks()` twice
    in a row, a moving cart after its move and again in its behaviour's tick (a bubble
    column's draw twice) (3.4 step 5; 4.0 item 8).
20. **Detector rails' entity half is missing** (`entityInside`, the scheduled check, own
    and direct signals, the comparator output from a container cart), and so is a
    comparator's reading of an item frame (`diode.rs` leaves it to entities) (3.0 item 7;
    3.8; 6.0 item 11; 6.6).

**Breaking, drops and unsaved state**

21. **A creative hit discards a vehicle** (no item, no `ENTITY_DIE`); damage is still
    added and sent first. The test reads the causing entity, so a creative player's arrow
    breaks a cart too. **A container vehicle spills its contents on any destroying
    removal**, creative included and whatever `doEntityDrops` says; the rule holds back
    only the vehicle's own item (3.0 items 9, 10; 4.10).
22. **Vehicles' `damage`, hurt time and direction are synched data, never saved**, and
    decay by one a tick. A survival player breaks a cart or boat with several hits
    (damage above 40, ten per point of attack); an arrival should carry the three, or a
    cart hit just before a line is whole after it. The same holds for other unsaved
    state a player could meet: `boardingCooldown` (60 ticks after dismounting, no
    remount), a boat's `outOfControlTicks` (ejection after 60 under water) and paddle
    phases, a stand's `lastHit`, a leash's angular momentum, the hanging entities' check
    counter (3.0 item 11; section 10).
23. **An armour stand breaks on two hits within 5 ticks of the world's game time**
    (`lastHit`, not saved): Clustine needs one game time all regions agree on. A creative
    player breaks it at once and **nothing drops, not even what it wears**; a survival
    break drops the stand and its equipment (5.0 item 4; 5.4).
24. **An item frame with an item takes two hits**: the first drops (creative: deletes)
    the item, the second breaks the frame; the frame drops before its item, and one
    `nextFloat` is drawn against `dropChance` (6.0 items 7, 8).
25. **Boats no longer break when they fall**, nor hurt their riders (**Inferred** for the
    riders) (4.0 item 3).
26. **Container carts roll according to their contents** (`0.98f + (15 − comparator
    signal)·0.001f`): the sim needs their items as items (3.0 item 12).
27. **TNT and hopper minecarts can be placed by their items**: explode or suck in items on
    their own. The plan chooses: unplaceable until N6, or placed inert with the
    difference named (3.0 item 13; 3.19).
28. **A powered activator rail ejects a plain cart's riders every tick** and sets its
    damage to 50 every ten ticks; the furnace cart draws `nextInt(4)` every fuelled tick,
    its push is as long as the player's distance, and it takes coal and charcoal
    (`#furnace_minecart_fuel`), 3600 ticks each, up to 32000 (3.0 items 14, 15; 3.17).

**Hanging entities and leashes**

29. **The direction is synched data** (index 8, `DIRECTION`); the add-entity packet
    carries the block's corner and the direction's 3D value; a painting's position codec
    is based on the block's corner (`trackingPosition`) (6.0 item 1; 6.8).
30. **Survival is checked every 101 ticks** (`ticksSinceLastCheck++ >= 100`, not saved),
    for frames, paintings, knots and cushions; **any non-zero move or push breaks** a
    hanging entity (a piston pops a painting) (6.0 items 3, 4).
31. **The painting variant is one `nextInt(n)` from the new painting's own random**,
    after its UUID's two `nextLong`, over the placeable variants of the largest area that
    fit, in `#minecraft:placeable`'s order; drawn even when the item names a variant
    (applied afterwards); nothing is placed if none fits (6.0 item 6; 6.4).
32. **A painting's entity chunk can differ from its block's**: its position is its box's
    centre, half a block off `block_pos` for even widths (6.0 item 9).
33. **A saved leash restores by UUID, or by a fence position for which a knot is found or
    created**, every tick for 100 ticks from the leashee's load, then drops a lead. A knot
    saves nothing but its block, discards itself when its last leashee goes, and a broken
    knot's leashees each drop a lead on their next tick, even when a creative player broke
    it (7.0 items 3 to 5).

**The protocol and the edge**

34. The protocol crate decodes none of `interact`, `attack`, `move_vehicle`,
    `paddle_boat`, `player_command`, `pick_item_from_entity`, and encodes none of
    `set_passengers` (109), `set_entity_link` (102), clientbound `move_vehicle` (58),
    `sound_entity`, `open_screen`; the edge drops those incoming packets silently today
    (2.10). `set_entity_data` needs the serializers of section 8.
35. **A vehicle with a player in it is tracked at the player's range** (**Inferred**,
    `ChunkMap` not read): the edge must not shrink its view to 10 or 8 chunks (1.0;
    section 8).
36. **No random draw** is made by riding, dismounting, `DismountHelper`, saving
    passengers, `ServerEntity`, the packet listener, `OldMinecartBehavior` or the leash's
    physics; the draws of N5 are in section 9.

---------------------------------------------------------------------------------------

## 1. Riding in `Entity`

### 1.0 Findings of this section

1. **The server seats a player first.** `Entity.addPassenger` (Entity.txt lines
   7088–7136) inserts a `Player` at index 0 when the vehicle's current first passenger
   is not a player (server side only); everyone else is appended. The first passenger is
   the one a boat lets steer (`AbstractBoat.getControllingPassenger`: the first passenger
   if it is a `Player`, else none, AbstractBoat.txt lines 2189–2210). So the order of
   `passengers` is state: an arrival and a `PlayerTransfer`'s "seat" must keep it, and a
   player seated after a pending vehicle arrives goes to index 0 if the first is not a
   player, not to the seat they had (ADR-0028 section 5 says "their seat"; the game has no
   seat number, only the order, rebuilt by this rule as each one mounts).
2. **A vehicle ridden by exactly one player is not saved with its chunk.**
   `Entity.shouldBeSaved` (lines 11340–11368) is false for a passenger, and false for a
   vehicle whose indirect passengers include exactly one `Player`
   (`hasExactlyOnePlayerPassenger`, lines 10255–10274, counts over all indirect
   passengers). In the game such a vehicle is saved with the player (**Inferred**:
   `ServerPlayer`/`PlayerList` write it as the player's root vehicle; not read) and goes
   with them when they leave (`RemovalReason.UNLOADED_WITH_PLAYER`, neither destroy nor
   save). ADR-0028 has the unit belong to the root's chunk always; with one player aboard
   the game instead takes the boat away with a player who logs out and brings it back
   when they log in. A decision for the plan: follow the game (the vehicle leaves the
   chunk with a player who leaves) or name the difference (the boat stays in the world).
   With two players aboard, or none, the vehicle is saved with its chunk, passengers
   nested in it.
3. **A passenger is never saved by its own chunk** (`save` returns false for a
   passenger, lines 5798–5808): it is written only inside its vehicle's `Passengers`
   list, recursively (lines 6006–6030). ADR-0028's "a unit is one record" holds.
4. **A riding entity is sent no position at all.** `ServerEntity.sendChanges`
   (ServerEntity.txt lines 114–559) sends a passenger only `MoveEntity.Rot` (when its
   angles changed by ≥ 1 step), rebases the position codec on its position every update,
   and sets `wasRiding`; the first move packet after it stops riding is therefore a whole
   `EntityPositionSync`. Clients place riders themselves (**Client**). The edge's
   `Tracker` (services/edge/src/tracking.rs) needs a "riding" flag per report.
5. **`SetPassengers` is sent by the vehicle's tracker on the tick its list changes**
   (compared with `List.equals` against the last sent, lines 120–148), to trackers only
   (not to the vehicle itself, which is not a player for these types), and at pairing
   both for the entity's own passengers and, if it rides, its vehicle's list
   (lines 869–1022).
6. **`boardingCooldown`**: dismounting sets the passenger's `boardingCooldown = 60`
   (`removePassenger`, line 7177); `baseTick` counts it down (lines 985–992); while it
   is above 0, or the entity is sneaking, `canRide` is false, so it cannot remount
   (unless `force`). It is not saved (`saveWithoutId` writes no such key), so in the game
   it is lost on unload. A crossing in Clustine should carry it (it is a "seconds-long"
   effect a player could notice only if a mob were made to remount; low stakes).
7. **Most of dismounting a player is not in these classes.** `Entity.stopRiding` is only
   `removeVehicle` (lines 7082–7086); the placing of a dismounted living entity is
   `LivingEntity.removeVehicle`/`dismountVehicle` (not read), which asks the vehicle's
   `getDismountLocationForPassenger` (the minecart's and the boat's overrides are read;
   below) and calls `dismountTo`. Needed before porting (Not read list).
8. **Who simulates a ridden vehicle.** No class here overrides
   `getMoveSimulationType` (all `AUTHORITATIVE_SIDE`); `isLocalInstanceAuthoritative` on
   the server is `!isClientAuthoritative()`, which is true only when
   `getControllingPassenger()` is non-null and that passenger `isClientAuthoritative()`
   (`LivingEntity`, not read; **Inferred**: true for a player). So a boat with a player in
   front is the client's; a boat with only a mob, any minecart, a cushion are the
   server's. `Entity.getControllingPassenger` returns null; only `AbstractBoat` overrides
   it among the N5 classes.
9. **On the server nothing can ride a player**: `startRiding` refuses a vehicle whose
   type `!canSerialize()` (lines 6912–6994, at 19–40), and **Inferred** the player type
   is not serialisable (`EntityType.PLAYER` is built with `noSave()`; not read).
10. **No random draw** is made anywhere in riding, dismounting, saving passengers or
    `ServerEntity` (section 9).

### 1.1 State

Fields: `vehicle` (an `Entity`, null when not riding), `passengers` (a Guava
`ImmutableList<Entity>`, `ImmutableList.of()` when none, replaced whole on every change,
never mutated), `boardingCooldown` (`int`, protected). The game holds references; in
Clustine both are keys inside one unit (ADR-0028 section 5), and every change is a new
list, which suits a commit.

Queries (Entity.txt):
- `isPassenger()` = `getVehicle() != null` (lines 7582–7590); `getVehicle()` = `vehicle`
  (10479–10483).
- `isVehicle()` = `!passengers.isEmpty()` (7592–7601).
- `getPassengers()` = the list itself (10167–10171); `getFirstPassenger()` = index 0 or
  null (10173–10186).
- `hasPassenger(Entity)` = `passengers.contains(e)` (10188–10194, identity by `equals`,
  which is by id, lines 671–690); `hasPassenger(Predicate)` = any direct passenger
  matches (10196–10217).
- `getIndirectPassengersStream()` = `passengers.stream().flatMap(...)` (10219–10226),
  **Inferred** `flatMap(Entity::getSelfAndPassengers)`, i.e. depth first, each
  passenger before its own passengers. `getSelfAndPassengers()` = this, then that
  (10228–10235). `getPassengersAndSelf()` = passengers' `getPassengersAndSelf` then this
  (10237–10247; post-order). `getIndirectPassengers()` iterates the first
  (10249–10253, 11682–11687).
- `countPlayerPassengers()` = count of `Player` among indirect passengers
  (10255–10263, 11676–11680); `hasExactlyOnePlayerPassenger()` = that `== 1`
  (10265–10274).
- `getRootVehicle()`: follow `getVehicle()` while `isPassenger()` (10276–10288).
  `isPassengerOfSameVehicle(e)` = same root (by identity, 10290–10300).
  `hasIndirectPassenger(e)`: walks `e`'s vehicle chain looking for this (10302–10320).
- `getControllingPassenger()` = null (10152–10155); `hasControllingPassenger()` = it is
  non-null (10157–10165). `getControlledVehicle()` = `vehicle` if
  `vehicle.getControllingPassenger() == this`, else null (10485–…).
- `canControlVehicle()` = not in entity tag `#minecraft:non_controlling_rider`
  (7610–…); `dismountsUnderwater()` = in `#minecraft:dismounts_underwater` (7603–7608).
  (Used by `LivingEntity` and the boat; not by `Entity`'s riding code.)
- `showVehicleHealth()` = `this instanceof LivingEntity` (6906–6910; a client HUD
  matter).
- `isFlyingVehicle()` = false (11377–11380; no N5 type overrides it).

### 1.2 `startRiding`

`startRiding(Entity v)` (final, lines 6897–6904) = `startRiding(v, false, true)`.

`startRiding(Entity v, boolean force, boolean sendEventAndTriggers)` (lines 6912–6994),
in order:
1. if `v == this.vehicle` → false.
2. if `!v.couldAcceptPassenger()` → false (`Entity`: true, 7187–7190; no N5 override
   among the classes read).
3. server only: if `!v.type.canSerialize()` → false (finding 9).
4. cycle check: `e = v; while (e.vehicle != null) { if (e.vehicle == this) return false;
   e = e.vehicle; }` (it does not test `v == this` itself).
5. if `!force`: if `!this.canRide(v) || !v.canAddPassenger(this)` → false.
   - `Entity.canRide(v)` = `!isShiftKeyDown() && boardingCooldown <= 0` (6996–7007).
     (`isShiftKeyDown` is shared flag bit 1.)
   - `Entity.canAddPassenger(p)` = `passengers.isEmpty()` (7180–7185). The boat: fewer
     than `getMaxPassengers()` (2; the chest boat and chest raft 1,
     AbstractChestBoat.txt line 30) and `!isEyeInFluid(#water)` (AbstractBoat.txt
     2167–2187). Minecarts and the cushion keep `Entity`'s (one passenger).
6. if `isPassenger()`: `stopRiding()` (leave the old vehicle; its `removePassenger` sets
   `boardingCooldown = 60` on this, which does not undo this mount).
7. `setPose(Pose.STANDING)` (marks `DATA_POSE` dirty if it changed).
8. `this.vehicle = v`; `v.addPassenger(this)` (1.3).
9. if `sendEventAndTriggers`: `level.gameEvent(this, GameEvent.ENTITY_MOUNT,
   v.position)`; for each `ServerPlayer` among `v`'s indirect passengers,
   `CriteriaTriggers.START_RIDING_TRIGGER.trigger(player)` (lambdas 11772–11784; an
   advancement, out of scope).
10. → true.

The position of the rider is not set here; it is set by the vehicle's next
`positionRider` (1.5). **Inferred** (not read): `ServerPlayer.startRiding` overrides this
to position the player at once and send them a teleport keeping their rotation; read
before porting a player's mount (Not read list).

Who calls it among the classes read: `Minecart.interact` (Minecart.txt lines 12–51:
not sneaking (`isSecondaryUseActive`), not already a vehicle; server: `player.startRiding
(this)` → `SUCCESS_SERVER` if it took, else `CONSUME`; sneaking or occupied → `PASS`);
`AbstractBoat.interact` (2051), `Cushion.interact` (Cushion.txt 151–176), and mobs picked
up by a moving cart or boat (`OldMinecartBehavior` 1414, `NewMinecartBehavior` 1806,
`AbstractBoat` 650): sections 3 to 5 give the rules.

### 1.3 `addPassenger` and `removePassenger`

`addPassenger(Entity p)` (protected, lines 7088–7136):
1. if `p.getVehicle() != this` → `IllegalStateException("Use x.startRiding(y), not
   y.addPassenger(x)")`.
2. if `passengers` is empty → `passengers = ImmutableList.of(p)`.
3. else `list = Lists.newArrayList(passengers)`; if server and `p instanceof Player` and
   `!(getFirstPassenger() instanceof Player)`: `list.add(0, p)`; else `list.add(p)`;
   `passengers = ImmutableList.copyOf(list)`.

`removePassenger(Entity p)` (protected, lines 7138–7178):
1. if `p.getVehicle() == this` → `IllegalStateException("Use x.stopRiding(y), not
   y.removePassenger(x)")` (the caller clears `p.vehicle` first).
2. if `passengers.size() == 1 && passengers.get(0) == p` → `ImmutableList.of()`; else
   `passengers` = the list without `p` (filter `e != p`, order kept).
3. `p.boardingCooldown = 60`.

`Cushion.removePassenger` (Cushion.txt 178–…): `super`, then on the server if the
cushion is not removed, `playSound(CUSHION_GET_UP, 1, 1)`.

### 1.4 `stopRiding`, `removeVehicle`, `ejectPassengers`, `unRide`, removal

- `stopRiding()` = `removeVehicle()` (7082–7086). (`LivingEntity` and `Player` override
  it, not read; see finding 7.)
- `removeVehicle()` (7051–7080): if `vehicle != null`: `old = vehicle; vehicle = null;
  old.removePassenger(this)`; then if `getRemovalReason() == null ||
  getRemovalReason().shouldDestroy()`: `level.gameEvent(this, ENTITY_DISMOUNT,
  old.position)`.
- `ejectPassengers()` (7009–7033): for `i` from `passengers.size() - 1` down to 0:
  `passengers.get(i).stopRiding()` (reads the field afresh each time, so it works on the
  shrinking list from the end).
- `unRide()` (final, 541–552): `if isVehicle() ejectPassengers(); if isPassenger()
  stopRiding()`.
- `baseTick` (963–…, lines 22–60 of the method): `if isPassenger() &&
  getVehicle().isRemoved(): stopRiding()`; then `if boardingCooldown > 0:
  boardingCooldown--`. (Before `handlePortal`, fluids and the rest that N4 ported.)
- `setRemoved(reason)` (final, 11299–11324): `if removalReason == null: removalReason =
  reason`; `if removalReason.shouldDestroy(): stopRiding()`; then **every passenger**
  `stopRiding()`s (`getPassengers().forEach(...)`, **Inferred**
  `Entity::stopRiding`), whatever the reason; then `levelCallback.onRemove(reason)`,
  `onRemoval(reason)`. `remove(reason)` = `setRemoved(reason)` (701–705); `discard()` =
  `remove(DISCARDED)` (660–665).
- `RemovalReason` (Entity$RemovalReason.txt): `KILLED` (destroy, not save),
  `DISCARDED` (destroy, not save), `UNLOADED_TO_CHUNK` (not destroy, save),
  `UNLOADED_WITH_PLAYER` (neither), `CHANGED_DIMENSION` (neither).

So a vehicle broken (killed or discarded) drops its passengers, which keep their
positions (**Inferred**: then `LivingEntity` places them, finding 7); a passenger killed
leaves its vehicle; the `ENTITY_DISMOUNT` game event is emitted by a passenger that is
not removed or is destroyed. A vehicle unloaded with its chunk also loses its passengers
in memory (after it was saved with them), which matters only to the game's own unload.

Changing dimension with passengers (`calculatePassengerTransition`, 8960–…, and the
teleport around 9046) is out of N5 (no portals); name only.

### 1.5 `rideTick`, `positionRider` and attachment points

`rideTick()` (6785–6800): `setDeltaMovement(Vec3.ZERO)`; `tick()`; `if isPassenger():
getVehicle().positionRider(this)`. So a passenger's motion is zeroed before its own
tick, it ticks fully (its `baseTick` included), then is put where its vehicle says.

`positionRider(Entity p)` (final, 6802–6813): `if !hasPassenger(p) return`;
`positionRider(p, Entity::setPos)` (**Inferred**: the `MoveFunction` of indy #8 is
`setPos(DDD)`).

`positionRider(Entity p, MoveFunction f)` (protected, 6815–6843):
```
riding  = this.getPassengerRidingPosition(p)        // vehicle side
attach  = p.getVehicleAttachmentPoint(this)          // passenger side
f.accept(p, riding.x - attach.x, riding.y - attach.y, riding.z - attach.z)
```
`getPassengerRidingPosition(p)` (6860–6871) = `position().add(getPassengerAttachmentPoint
(p, dimensions, 1.0f))` (the scale `1.0f` is a constant here).

`getPassengerAttachmentPoint(p, dims, scale)` (6873–6880) =
`getDefaultPassengerAttachmentPoint(this, p, dims.attachments())` (6882–6895):
`i = getPassengers().indexOf(p)`; `attachments.getClamped(EntityAttachment.PASSENGER, i,
this.yRot)`.

`getVehicleAttachmentPoint(vehicle)` (6849–6858) = `getAttachments().get(
EntityAttachment.VEHICLE, 0, this.yRot)` (the passenger's own type's attachment).

`EntityAttachments.get/getClamped` and the per-type points live in `EntityType` and
`EntityAttachments` (not read). **Inferred** from the game's design: `getClamped` clamps
the index to the list; `get` throws on a missing index; the point is rotated about Y by
the entity's `yRot` (`Vec3.yRot(-yRot·π/180)`); a type that declares none gets
`PASSENGER = (0, height, 0)` and `VEHICLE = (0, 0, 0)`; the player's vehicle attachment
is `(0, 0.6, 0)`... none of this can be ported from what is read: dump `EntityType`
(builder calls in its static initialiser), `EntityAttachments`, `EntityAttachment` and
`Player` (Not read list).

Overrides among the N5 classes:
- `AbstractMinecart.getPassengerAttachmentPoint` (AbstractMinecart.txt 180–201): a
  `Villager` or `WanderingTrader` passenger gets `LOWERED_PASSENGER_ATTACHMENT =
  (0, 0, 0)` (static init, 1525–1533); anything else the default (via
  `VehicleEntity`, which does not override it).
- `Minecart.positionRider` (Minecart.txt 132–176): `super`, then a client-only rotation
  of a player passenger under the experimental behaviour (**Client**); nothing on the
  server.
- `AbstractBoat.getPassengerAttachmentPoint` (171–…), `positionRider` (1756–…),
  `onPassengerTurned` (1994–…): section 4.6 covers them (seat offsets 0.2 and −0.6,
  +0.2 for an `Animal`, turning the rider with the boat).
- `onPassengerTurned(p)` = nothing in `Entity` (6845–6847).

**The tick order of passengers is in `ServerLevel`** (`tickNonPassenger`,
`tickPassenger`; not read). **Inferred** from the game: after a root ticks
(`commonTick` + `tick`), each of its passengers, in list order, is ticked by
`tickPassenger(vehicle, passenger)`: if the passenger is removed or no longer rides that
vehicle it `stopRiding()`s; else (if it is a `Player` or in the tick list) it
`commonTick()`s and `rideTick()`s, then its own passengers recursively. A passenger is
never ticked as a root. A player's `rideTick` on the server comes from `ServerPlayer`
(not read). Read `ServerLevel` before porting the order (Not read list).

### 1.6 Authority: who moves a ridden vehicle

(Entity.txt 10322–10408)
- `isLocalInstanceAuthoritative()` (final): on the server `!isClientAuthoritative()`.
- `isClientAuthoritative()` = `getControllingPassenger() != null &&
  getControllingPassenger().isClientAuthoritative()` (the `LivingEntity` method, not
  read).
- `getMoveSimulationType()` = `AUTHORITATIVE_SIDE` (no override in the classes read).
- `canSimulateMovement()` (final): `SERVER_AND_CLIENT` → true; `AUTHORITATIVE_SIDE` →
  `isLocalInstanceAuthoritative()`; `AUTHORITATIVE_SIDE_AND_SERVER` → true on the server
  (the switch map, Entity$1.txt).
- `isEffectiveAi()` = `isLocalInstanceAuthoritative()` (10410–…).
- `Entity.move` uses `isLocalInstanceAuthoritative()` for the vertical-collision flags
  (n4-brief section 1.2 step 9): for a client-driven boat the server does not set
  `verticalCollision`/`onGround` from its own move; section 4.2 covers what the
  server computes then.
- `recordMovement(type, v)` (1840–1856) feeds the interpolation handler only for movers
  that are server-and-client simulated or `SERVER_AND_CLIENT` types: not N5's vehicles.

### 1.7 Dismounting: `getDismountLocationForPassenger` and `DismountHelper`

`Entity.getDismountLocationForPassenger(living)` (10465–10477) = `(getX(),
getBoundingBox().maxY, getZ())`: on top of the vehicle's box. `VehicleEntity` does not
override it.

`AbstractMinecart.getDismountLocationForPassenger(living)` (AbstractMinecart.txt
203–442):
1. `dir = getMotionDirection()` (old behaviour: `getDirection().getClockWise()`, or
   `getDirection().getOpposite().getClockWise()` when `isFlipped()`,
   OldMinecartBehavior.txt 1452–1468; `getDirection()` = `Direction.fromYRot(yRot)`,
   Entity.txt 9947–9953). If `dir.getAxis() == Y` (cannot happen with `fromYRot`) →
   `super`.
2. `offsets = DismountHelper.offsetsForDirection(dir)`; `base = blockPosition()`.
3. For each pose in `living.getDismountPoses()` (`LivingEntity`, not read; **Inferred**
   `[STANDING, CROUCHING, SWIMMING]` for a player... check): `dims =
   living.getDimensions(pose)`; `half = Math.min(dims.width(), 1.0f) / 2.0f` (float);
   for each `dy` in `POSE_DISMOUNT_HEIGHTS[pose]` (`STANDING: [0, 1, -1]`, `CROUCHING:
   [0, 1, -1]`, `SWIMMING: [0, 1]`, static init 1534–1565): for each `[ox, oz]` in
   `offsets`:
   - `pos = (base.x + ox, base.y + dy, base.z + oz)`;
   - `floor = level.getBlockFloorHeight(DismountHelper.nonClimbableShape(level, pos),
     () -> DismountHelper.nonClimbableShape(level, pos.below()))`;
   - if `!DismountHelper.isBlockFloorValid(floor)` → next;
   - `box = AABB(-half, 0, -half, half, dims.height(), half)` (each `f2d`'d);
     `at = Vec3.upFromBottomCenterOf(pos, floor)` (`(pos.x + 0.5, pos.y + floor,
     pos.z + 0.5)`);
   - if `DismountHelper.canDismountTo(level, living, box.move(at))`:
     `living.setPose(pose)`; return `at`.
4. Nothing fitted: `maxY = getBoundingBox().maxY`; `m.set((double) base.x, maxY,
   (double) base.z)` (`MutableBlockPos.set(DDD)` floors each); for each pose in
   `getDismountPoses()`: `h = (double) dims(pose).height()`; `n = Mth.ceil(maxY - m.y +
   h)`; `ceiling = DismountHelper.findCeilingFrom(m, n, p -> level.getBlockState(p)
   .getCollisionShape(level, p))` (lambda 1504–1514); if `maxY + h <= ceiling`:
   `living.setPose(pose)`; break. Then return `super` (`(x, maxY, z)`).

The boat's (AbstractBoat.txt 1790–…) is in section 4.6.

`DismountHelper` (DismountHelper.txt, whole class):
- `offsetsForDirection(d)` (9–166): with `cw = d.getClockWise()`, `ccw =
  cw.getOpposite()`, `back = d.getOpposite()`, eight `[x, z]` offsets in this order:
  `cw`, `ccw`, `back + cw`, `back + ccw`, `d + cw`, `d + ccw`, `back`, `d` (each the
  directions' `getStepX`/`getStepZ` summed).
- `isBlockFloorValid(h)` (168–180) = `!Double.isInfinite(h) && h < 1.0`.
- `canDismountTo(level, living, box)` (182–213): false if any of
  `level.getBlockCollisions(living, box)` is non-empty; false if
  `!level.getWorldBorder().isWithinBounds(box)`; else true. (Block collisions only,
  with the entity's collision context; no entities.)
- `canDismountTo(level, at, living, pose)` (215–225) = the above with
  `living.getLocalBoundsForPose(pose).move(at)`.
- `nonClimbableShape(level, pos)` (227–253): the state's collision shape, or empty if
  the state is in `#minecraft:climbable` or is an open trapdoor (`TrapDoorBlock` with
  `OPEN`).
- `findCeilingFrom(pos, n, shapeAt)` (255–290): for `i` in `0..n`: `s =
  shapeAt(pos + i up)`; if non-empty → return `pos.y + i + s.min(Y)`; after `n` →
  `+∞`.
- `findSafeDismountLocation(type, level, pos, checkDanger)` (292–389): if `checkDanger`
  and `type.isBlockDangerous(level.getBlockState(pos))` → null; `floor =
  level.getBlockFloorHeight(nonClimbableShape(pos), () -> nonClimbableShape(pos.below()))`;
  invalid → null; if `checkDanger && floor <= 0` and the block below is dangerous → null;
  `at = upFromBottomCenterOf(pos, floor)`; `box = type.getDimensions().makeBoundingBox(at)`;
  any non-empty `getBlockCollisions(null, box)` → null; if `type == PLAYER` and the
  state at `pos` or `pos.above()` is in `#minecraft:invalid_spawn_inside` → null; outside
  the world border → null; else `at`. (Used by `LivingEntity`'s and the boat's
  dismounts.)

`BlockGetter.getBlockFloorHeight(shape, below)` is not in these classes. **Inferred**:
`shape.isEmpty() ? (b = below.get().max(Y)) >= 1.0 ? b - 1.0 : -∞ : shape.max(Y)`;
dump `BlockGetter` (Not read list). `EntityType.isBlockDangerous` likewise.

`dismountTo(x, y, z)` (9645–9651) = `teleportTo(x, y, z)` (9653–…: on a `ServerLevel`,
`snapTo(x, y, z, yRot, xRot)` then `teleportPassengers()`, which repositions every
indirect passenger through `positionRider` with **Inferred** `Entity::snapTo`-like
`MoveFunction` (indy #26, lambda 11689–11707)). `ServerPlayer` overrides `dismountTo`
and `teleportTo` (not read; **Inferred** it sends the player a teleport).

### 1.8 Pushing between riders

`Entity.push(Entity other)` (5187–5252): nothing if `isPassengerOfSameVehicle(other)`
(a unit never pushes itself) or either is `noPhysics`; `dx = other.x - x`, `dz =
other.z - z`, `m = Mth.absMax(dx, dz)`; if `m >= 0.009999999776482582` (`0.01f`
widened): `m = sqrt(m)`; `dx /= m; dz /= m`; `k = min(1/m, 1)`; `dx *= k; dz *= k`;
`dx *= 0.05000000074505806; dz *= …` (`0.05f` widened); if `!this.isVehicle() &&
this.isPushable()`: `this.push(-dx, 0, -dz)`; if `!other.isVehicle() &&
other.isPushable()`: `other.push(dx, 0, dz)`. **A vehicle with passengers is never pushed
by this.** `push(x, y, z)` (5276–…): if all finite, `deltaMovement += (x, y, z)` and
`needsSync = true`. (Minecarts and boats override `push(Entity)`; sections 3.10 and 4.9 cover it.)

`propagateFallToPassengers(d, f, src)` (4067–4086): every direct passenger
`causeFallDamage(d, f, src)` (called from `causeFallDamage` of a vehicle; the boat's
fall rule).

### 1.9 What is saved

- `save(out)` (5798–5808): a passenger → false (not written); else `saveAsPassenger`.
- `saveAsPassenger(out)` (5770–5796): false if removed with a reason that does not save;
  `id = getEncodeId()` (6338–6356: null if `!type.canSerialize()`, else the type's key as
  a string); null → false; `out.putString("id", id)`; `saveWithoutId(out)`; true.
- `saveWithoutId(out)` (5810–6034): first `"Pos"`: for a passenger **`(vehicle.getX(),
  this.getY(), vehicle.getZ())`**, else its position; then `Motion`, `Rotation`,
  `fall_distance`, `Fire`, `Air`, `OnGround`, `Invulnerable`, `PortalCooldown`,
  `invulnerable_time` (if > 0), `UUID`, `CustomName`, `CustomNameVisible`, `Silent`,
  `NoGravity`, `Glowing`, `TicksFrozen`, `HasVisualFire`, `Tags`, `data`; then
  `addAdditionalSaveData(out)`; then if `isVehicle()`: a list `"Passengers"`, one child
  per direct passenger in order, each `saveAsPassenger(child)`, a child that returns
  false discarded (`discardLast`), the list discarded if it ends empty. Not saved:
  `boardingCooldown`, the vehicle's key (a passenger is known by being in the list).
- Loading passengers (the recursion that rebuilds the unit and calls
  `startRiding(vehicle, true, …)`) is `EntityType.loadEntityRecursive` (not read).
  **Inferred**: each child is loaded and made to ride its parent with `force = true`, in
  list order, so `addPassenger`'s player-first rule does not reorder non-players.
- `shouldBeSaved()` (11340–11368): false if removed with a reason that does not save;
  false if a passenger; false if a vehicle with exactly one player among its indirect
  passengers; else true (finding 2).

Entity tags: `Passengers` (`TAG_PASSENGERS`), `id`, `UUID` (constants, lines 5–9).

### 1.10 Leashes in `Entity` (name only; section 7 covers `Leashable`)

`baseTick` calls `Leashable.tickLeash(level, this)` for a `Leashable` (1096–1102);
`interact` (6475–~6680) handles leading: sneaking with leashed mobs around → tie them to
this (`leashableInArea`, `canHaveALeashAttachedTo`, `setLeashedTo`), shears →
`shearOffAllLeashConnections` (6681–6712) / `dropAllLeashConnections` (6714–…), holder
clicking its own leashed entity → unleash (creative `removeLeash`, else `dropLeash`), a
lead in hand → leash it; `getQuadLeashHolderOffsets` (11069–…),
`supportQuadLeashAsHolder` (11079–…), `notifyLeashHolder`/`notifyLeasheeRemoved`
(11084–11091); the removal for dimension change calls `removeLeash` (9228–9245).
`Leashable.tickLeash` can call `stopRiding` (Leashable.txt 894).

### 1.11 What `ServerEntity` sends for riders and the N5 types (for the edge)

ServerEntity.txt, whole class; n4-brief section 9 has the common path, which stands.
What N5 adds:

#### 1.11.1 Per tick, `sendChanges()` (lines 114–559)

```
entity.updateDataBeforeSync()
ps = entity.getPassengers()
if !ps.equals(lastPassengers):                       // List.equals: same entities, same order
    sendToTrackingPlayers(SetPassengers(entity)); lastPassengers = ps
if entity is ItemFrame && tickCount % 10 == 0:
    if its item is a MapItem with saved data: for every player in the level: data.tickCarriedBy(player, stack, frame);
        send that player data.getUpdatePacket(mapId, player) if not null   // maps: N6 or later
    sendDirtyEntityData()
pos = entity.trackingPosition()                      // Painting overrides it (section 6.4)
interpolationTracker.updateTracking(pos)
if entity.syncPosition: tickCount = updateInterval.nextInterval(tickCount); syncPosition = false
if entity.needsSync || updateInterval.test(tickCount) || entityData.isDirty():
    yRot, xRot = packDegrees(...); rot = |yRot - lastSentYRot| >= 1 || |xRot - lastSentXRot| >= 1
    if entity.isPassenger():                         // riding: no position, no motion
        if rot: send MoveEntity.Rot(id, yRot, xRot, onGround); lastSent rotations = these
        interpolationTracker.clear(); positionCodec.setBase(pos)
        sendDirtyEntityData(); wasRiding = true
    else if entity is AbstractMinecart with NewMinecartBehavior:
        handleMinecartPosRot(behaviour, yRot, xRot, rot)       // experimental only, 2.3
    else:
        (the n4 path: teleportDelay++, path = interpolationTracker.getPositionPath(pos),
         interpolationTracker.clear(); arrows force pos and rot; motion if needsSync or
         trackDelta (or fall flying); move packet; data; bases)
        wasRiding = false
    head rotation (RotateHead if it changed by >= 1)
    entity.needsSync = false
tickCount++
if entity.syncVelocity: syncVelocity = false; sendToTrackingPlayersAndSelf(SetEntityMotion(entity))
```

Notes:
- `createMovePacket` (560–653) sends the whole `EntityPositionSync` when `wasRiding`: the
  first update after dismounting is absolute. It also sends it when `teleportDelay >
  400`, on a change of `onGround`, or when `getRequiresPrecisePosition()`.
- The gate is the type's `updateInterval` (n4: `test(t) = t % n == 0`; **Inferred**
  for `UpdateInterval` with `n = Integer.MAX_VALUE`: true only at `t == 0`), so a passenger's
  rotation goes out on update ticks only (every 3 ticks for minecarts and boats, 2 for
  players and mannequins), or when its data is dirty or `needsSync` is set.
- The vehicle under a client's control is sent to every tracker by the same code as any
  other entity (`sendToTrackingPlayers`, which leaves out only the entity itself);
  **Client**: the controlling client ignores moves for the entity it controls. The move
  of a ridden vehicle comes from `handleMoveVehicle` (section 2.3).
- `isFullPrecisionEncodingRequired` (655–710) is for `ItemEntity` only.
- `sendDirtyEntityData()` (1058–1116): `packDirty()`; if non-null,
  `trackedDataValues = getNonDefaultValues()` and `SetEntityData` to trackers and self;
  for a `LivingEntity` (armour stands, mannequins) the attributes to sync →
  `UpdateAttributes` to trackers and self, then cleared.

#### 1.11.2 Pairing: `addPairing` and `sendPairingData` (842–1022)

`addPairing(player)`: collects `sendPairingData` into a list, sends one
`ClientboundBundlePacket` of them, then `entity.startSeenByPlayer(player)`.
`removePairing(player)` (820–840): `entity.stopSeenByPlayer(player)`, then
`RemoveEntities([id])`.

`sendPairingData(player, out)`, in order:
1. `entity.updateDataBeforeSync()`; (a warning if removed);
2. `entity.getAddEntityPacket(this)` (Entity: `ClientboundAddEntityPacket(entity,
   serverEntity)`, Entity.txt 10682–10689; overridden by `Painting`, `ItemFrame`,
   `LeashFenceKnotEntity`: sections 6.8 and 7.6);
3. if `trackedDataValues != null`: `SetEntityData(id, trackedDataValues)` (the
   non-default values as of the last send, not of now);
4. a `LivingEntity` with syncable attributes: `UpdateAttributes(id, them)`;
5. a `LivingEntity`: `SetEquipment(id, [(slot, stack.copy())...])` for every
   `EquipmentSlot.VALUES` slot that is not empty, if any (armour stands, mannequins);
6. if `getPassengers()` is not empty: `SetPassengers(entity)`;
7. if `isPassenger()`: `SetPassengers(getVehicle())` (so a client that meets a rider
   before its vehicle is told again when it meets the vehicle, and the other way round);
8. a `Leashable` that `isLeashed()`: `SetEntityLink(entity, getLeashHolder())`.

Wire (generated `packets.json`, play, clientbound): `set_passengers` 109 (varint
vehicle id, varint array of direct passenger ids in list order; **Inferred** from the
packet's constructor taking the entity), `set_entity_link` 102, `entity_position_sync`
35, `move_minecart_along_track` 56 (experimental only), `set_entity_motion` 103.

#### 1.11.3 `handleMinecartPosRot` (712–818): experimental behaviour only

`sendDirtyEntityData()`; if `behaviour.lerpSteps` is empty: if the position moved (codec
delta `>= 7.62939453125E-6` squared) or `tickCount % 60 == 0`, or `rot`, or the motion
moved from `lastSentMovement` by more than `1e-7` squared: `MoveMinecartAlongTrack(id,
[MinecartStep(position, motion, yRot, xRot, 1.0f)])`; else send all `lerpSteps` and clear
them; then the last-sent rotations and the codec base are set. Not needed while
`minecart_improvements` is off (section 3.9).

#### 1.11.4 Interpolation trackers

`AbstractMinecart.createInterpolationHandler()` (AbstractMinecart.txt 659–669):
`NO_OP` under the experimental behaviour, else `LinearInterpolationHandler.create(this)`;
`AbstractBoat`'s (355–361) is `LinearInterpolationHandler.create(this, 3)`. What the
server-side `interpolationTracker()` of a `LinearInterpolationHandler` gives
`getPositionPath` (a linear path, or steps that change the `VecDelta`'s step count) is
not in these classes: dump `LinearInterpolationHandler` and `InterpolationTracker`
(Not read list). The client's `commonTick` interpolates (Entity.txt 935–961; **Client**).

---------------------------------------------------------------------------------------

## 2. The player's side: `ServerGamePacketListenerImpl`

Line ranges in this section are of `ServerGamePacketListenerImpl.txt` unless another
class is named. Packet ids are 26.3's (`generated/reports/packets.json`, play state).

### 2.0 Findings of this section

1. **Boats are still driven by the client in 26.3.** `AbstractBoat.getControllingPassenger`
   (AbstractBoat.txt 2189–2205) is the first passenger if it is a `Player`, else
   `VehicleEntity`'s (none); `Entity.isLocalInstanceAuthoritative` on the server is
   `!isClientAuthoritative()`, true unless the controlling passenger is a client-driven
   living entity (Entity.txt 10322–10353). The controlling client sends
   `ServerboundMoveVehiclePacket` (`move_vehicle`, serverbound 34: position as three
   doubles, `yRot`, `xRot` floats, **`onGround` boolean**, new against 1.21.1) and
   `ServerboundPaddleBoatPacket` (`paddle_boat`, 35: two booleans, left and right); the
   server checks the move (2.3) and answers only with `ClientboundMoveVehiclePacket`
   (`move_vehicle`, clientbound 58: position, `yRot`, `xRot`) when it refuses it.
   **Minecarts have no controlling passenger** (no override of
   `Entity.getControllingPassenger`, Entity.txt 10152–10155, which returns null): the
   server moves them; a rider's keys reach a cart only through
   `ServerboundPlayerInputPacket` → `ServerPlayer.setLastClientInput` →
   `ServerPlayer.getLastClientMoveIntent()`, which `OldMinecartBehavior` reads
   (OldMinecartBehavior.txt 450–456, section 3.6).
2. **26.3 split the old use-entity packet in two.** `ServerboundAttackPacket` (`attack`,
   serverbound 1: the entity id) is handled by `handleAttack` (5460–5565), and
   `ServerboundInteractPacket` (`interact`, 26: entity id, hand, location,
   `usingSecondaryAction`) by `handleInteract` (5566–5679). There is no
   `ServerboundInteractPacket.Handler`, no `onInteraction`/`onAttack`, and **no
   `interactAt`**: `Entity.interact(Player, InteractionHand, Vec3 location)` takes the
   hit location itself (Entity.txt 6475; overridden by `ArmorStand` 367, `ItemFrame`
   827, `AbstractBoat` 2024, `Minecart` 15, `MinecartFurnace` 208, `MinecartChest` 65,
   `AbstractMinecartContainer` 122, `MinecartCommandBlock` 127, `AbstractChestBoat` 88,
   `LeashFenceKnotEntity` 128, `Cushion` 150). The armour stand's part chosen by the
   height of the click comes from that `location` (section 5.3).
3. **Interacting sets the sneak state.** `handleInteract` calls
   `player.setShiftKeyDown(packet.usingSecondaryAction())` before anything else is
   checked (5566+ offsets 44–52), so `Entity.canRide` (`!isShiftKeyDown() &&
   boardingCooldown <= 0`, Entity.txt 6996–) and `Player.isSecondaryUseActive` see the
   packet's own flag, not the last `PlayerInput`. The edge today keeps `sneaking` from
   `PlayerInput` only (`services/edge/src/play.rs:166-167`).
4. **The vertical error of a vehicle's move is never counted** (as for the player's
   own move): the bytecode is `if (dy > -0.5 || dy < 0.5) dy = 0` (offsets 454–473,
   lines 884–895), true for every finite `dy`. Only the horizontal error, squared
   above `0.0625`, makes a move "wrong".
5. **A riding player's own move packets move nothing.** `handlePlayerPositionChange`
   (3019–) for a passenger keeps the player's position and takes only the rotations
   (2.5). The rider's position is the vehicle's to set (`positionRider`, Entity part).
6. **The first `MoveVehicle` after mounting (or after a hand-over) is answered with a
   correction** unless the per-connection state is carried: `handleMoveVehicle` takes
   the controlling branch only if the root vehicle `== lastVehicle`, which
   `tickPlayer` sets at the end of the player's tick (2.2). A move that arrives before
   that falls into the "not controlling" branch, and since `vehiclePositionLastResetAt`
   is far behind `tickCount`, a `ClientboundMoveVehiclePacket` is sent at once (**Inferred**
   from the order; to confirm with a real client). For Clustine this matters at every
   crossing: a region that seats a rider must seed `lastVehicle` and
   `vehicleFirst/LastGood` (section 10), or the boat jumps back, which is what ADR-0028
   section 5's kept `MoveVehicle` is meant to prevent.
7. **A vehicle floated by its client kicks the player** after
   `getMaximumFlyingTicks(vehicle)` = `ceil(80 * max(0.08 / gravity, 1))` ticks (boats
   and minecarts, gravity `0.04`: 160 ticks), unless `server.allowFlight()`, the
   vehicle `isFlyingVehicle()` or `isNoGravity()` (2.2, 2.3). A creative player rowing
   a boat off a cliff edge in the air meets it; Clustine kicks nobody for flying today,
   so this is a named difference or a rule to port.
8. The protocol crate decodes none of the serverbound packets of this section beyond
   `PlayerInput`, `AcceptTeleportation`, the four player moves, `UseItemOn`, `UseItem`
   and `ClientTickEnd` (`crates/clustine-protocol/src/packets/play.rs:2161-2181`), and
   encodes none of `move_vehicle`, `set_passengers`, `set_entity_link`, `sound_entity`,
   `open_screen` (table in 2.10).

### 2.1 Packets and the order things happen in

Packets are run on the server thread (`PacketUtils.ensureRunningOnSameThread` at the
top of every handler), between ticks (**Inferred**: `MinecraftServer` runs queued tasks
while it waits for the next tick; not in the classes read). Then the levels tick (the
vehicle's own `tick`), then each connection's `tick()` (199–268) runs `tickPlayer`
(270–462) (N4 brief, finding 7). So in one server tick: the client's moves of the
previous client tick are applied and checked against the positions at the end of the
last `tickPlayer`; then the vehicle ticks; then `tickPlayer` records the new "good"
positions.

The client (**Client**, from the game's client code as known, not read here; confirm
with a real client): a riding player sends each client tick
`ServerboundMovePlayerPacket.Rot` (or the status-only form) with its look, then, if
its root vehicle is driven by it (`isLocalInstanceAuthoritative` on the client: a boat
whose first passenger it is), `ServerboundMoveVehiclePacket` of the root, and its
`ServerboundPlayerInputPacket` when the keys change, and `ServerboundPaddleBoatPacket`
when the paddles' state changes; then `ServerboundClientTickEndPacket`.

### 2.2 `tick()` and `tickPlayer()` as they bear on vehicles (199–462)

`tick()`: send `BlockChangedAck` if one is pending; unless paused, `tickPlayer()`; if
that returns true (kicked) return; then keep-alive, throttlers, idle kick.

`tickPlayer()` (270–462), in order:
1. `resetPosition()` (496–): `firstGood = lastGood = player position`.
2. `player.xo/yo/zo = position`; `player.doTick()` (the player's own tick, where a
   rider's `rideTick` and the shift-dismount live: `ServerPlayer`/`Player`, not read).
3. `player.absSnapTo(firstGoodX, firstGoodY, firstGoodZ, yRot, xRot)`.
4. `tickCount++`; `knownMovePacketCount = receivedMovePacketCount`.
5. The player's own floating kick: if `clientIsFloating && !isSleeping() &&
   !isPassenger() && !isDeadOrDying()`: `++aboveGroundTickCount >
   getMaximumFlyingTicks(player)` → disconnect `multiplayer.disconnect.flying`, return
   true; else clear `clientIsFloating` and `aboveGroundTickCount`. **A passenger's own
   floating counter is cleared every tick.**
6. `lastVehicle = player.getRootVehicle()`. If `lastVehicle == player` or
   `lastVehicle.getControllingPassenger() != player`: `lastVehicle = null`,
   `clientVehicleIsFloating = false`, `aboveGroundVehicleTickCount = 0`; return false.
7. Else: `vehicleFirstGood = vehicleLastGood = lastVehicle position`. If
   `clientVehicleIsFloating && lastVehicle.getControllingPassenger() == player`:
   `++aboveGroundVehicleTickCount > getMaximumFlyingTicks(lastVehicle)` → log "was
   kicked for floating a vehicle too long!", disconnect `multiplayer.disconnect.flying`,
   return true. Otherwise clear both. Return false.

`getMaximumFlyingTicks(entity)` (463–485): `g = entity.getGravity()`; if `g <
9.999999747378752E-6` (`1.0E-5F` widened) return `Integer.MAX_VALUE`; else
`Mth.ceil(80.0 * Math.max(0.08 / g, 1.0))`. Note the counter is incremented only when
the previous `MoveVehicle` left `clientVehicleIsFloating` set, and cleared on any tick
where it was not: one move on the ground resets it.

### 2.3 `handleMoveVehicle(ServerboundMoveVehiclePacket)` (669–1109)

`p` = the packet's position, `yRot`, `xRot`, `onGround`.

1. `containsInvalidValues(x, y, z, yRot, xRot)` (631–652: any coordinate NaN, or a
   rotation not finite) → disconnect `multiplayer.disconnect.invalid_vehicle_movement`
   (line 696).
2. If `updateAwaitingTeleport()` (3698–3739) or `!hasClientLoaded()` → return (the
   move is dropped). `updateAwaitingTeleport`: if `awaitingPositionFromClient != null`:
   if `tickCount - awaitingTeleportTime > 20`, set `awaitingTeleportTime = tickCount`
   and `teleport(awaiting position, player yRot, xRot)` again (a new id); return true.
   Else `awaitingTeleportTime = tickCount`, return false. `hasClientLoaded()` =
   `!waitingForRespawn && clientLoadedTimeoutTimer <= 0`.
3. `root = player.getRootVehicle()`. **Controlling branch** iff `root != player &&
   root.getControllingPassenger() == player && root == lastVehicle`:
   1. `(x0, y0, z0)` = root's position now. `x = clampHorizontal(p.x)`, `y =
      clampVertical(p.y)`, `z = clampHorizontal(p.z)` (653–668: `Mth.clamp` to
      `±3.0E7` and `±2.0E7`); `yRot = Mth.wrapDegrees(p.yRot)`, `xRot =
      Mth.wrapDegrees(p.xRot)` (floats).
   2. **Too quickly** (lines ~775–809): `d = (x, y, z) - vehicleFirstGood`; `m =
      root.getDeltaMovement().lengthSqr()`; `dd = dx*dx + dy*dy + dz*dz` (Java order:
      `(dx*dx + dy*dy) + dz*dz`); if `dd - m > 100.0` and `!isSingleplayerOwner()`:
      log "moved too quickly", `send(ClientboundMoveVehiclePacket.fromEntity(root))`,
      **return** (the vehicle is not touched; its rotation is not taken).
   3. `oldBox = root.getBoundingBox()`. `d = (x, y, z) - vehicleLastGood`. `wasBelow =
      root.verticalCollisionBelow` (read **before** the move). If root is a
      `LivingEntity` on a climbable: `resetFallDistance()` (not N5's vehicles).
   4. `root.move(MoverType.PLAYER, d)` (line 866): the full `Entity.move` with
      collision, step-up (`maxUpStep` of the vehicle), fall, block effects recording.
      `requestedDy = d.y` is kept.
   5. **Moved wrongly** (lines ~880–925): `e = (x, y, z) - root position after the
      move`; `e.y` set to 0 always (finding 4); `ee = ex*ex + 0 + ez*ez` (computed as
      `(ex*ex + ey*ey) + ez*ez` with `ey = 0.0`); `wrongly = ee > 0.0625`; log if so.
   6. **Refuse** if `(wrongly && level.noCollision(root, oldBox)) ||
      isEntityCollidingWithAnythingNew(level, root, oldBox, x, y, z)` (lines 940–971):
      `root.absSnapTo(x0, y0, z0, yRot, xRot)` (back to where it was, but **with the
      client's rotations**), `send(ClientboundMoveVehiclePacket.fromEntity(root))`,
      `root.removeLatestMovementRecording()` (the movement the check's `move` recorded
      is taken back, so its blocks are not applied), return. `noCollision(entity, box)`
      is the level's (blocks, the world border, and entities that `canCollideWith`:
      boats and minecarts collide with boats; not read here, `CollisionGetter`): a
      vehicle stuck in a block before the move is let through even when "wrong".
   7. **Accept** (lines ~975–1090): `root.absSnapTo(x, y, z, yRot, xRot)`: **the
      client's position, not the collided one**; `absSnapTo(DDD)` clamps x and z to
      `±3.0E7`, sets `xo/yo/zo` to the new position as well, then `setPos`;
      `absSnapRotationTo` sets `yRot = yRot % 360f`, `xRot = clamp(xRot, -90, 90) %
      360f` and the old rotations to the same (Entity.txt 4856–4925).
      `chunkSource.move(player)` (the player's own position is unchanged here: the
      rider is placed by the vehicle's next `positionRider`, **Inferred**).
      `moved = root position - (x0, y0, z0)`; `handlePlayerKnownMovement(moved)`
      (6731–6762: if `moved.lengthSqr() > 9.999999747378752E-6`
      `player.resetLastActionTime()`; `player.setKnownMovement(moved)`;
      `receivedMovementThisTick = true`); `root.setOnGroundWithMovement(p.onGround,
      moved)` (the client's on-ground, with `horizontalCollision` left as the move set
      it); `root.doCheckFallDamage(moved.x, moved.y, moved.z, p.onGround)`;
      `player.checkMovementStatistics(moved.x, moved.y, moved.z)` (statistics only).
   8. `clientVehicleIsFloating = requestedDy >= -0.03125 && !wasBelow &&
      !server.allowFlight() && !root.isFlyingVehicle() && !root.isNoGravity() &&
      noBlocksAround(root)` (lines 1030–1061). `noBlocksAround(e)` (1110–1129):
      `box = e.getBoundingBox().inflate(0.0625).expandTowards(0, -0.55, 0)`;
      `level.findBlocksIn(box).filterState(<predicate>).allMatched()`. The predicate is
      an `invokedynamic` whose target the dump does not show; **Inferred**: the game's
      `BlockStateBase::isAir`, so "no blocks" means every state in the box is air.
   9. `vehicleLastGood = root position`.
4. **Not controlling** (lines 1063–1109): if `root != player &&
   root.getControllingPassenger() != player` (a passenger who is not the driver, or a
   rider of a minecart whose client sent a move anyway) and (`tickCount -
   vehiclePositionLastResetAt > 20` **or** `root.distanceToSqr(p.position) > 1`, the
   bytecode's `||`): log "was expected to be controlling vehicle", `send(
   ClientboundMoveVehiclePacket.fromEntity(root))`, `vehiclePositionLastResetAt =
   tickCount`. A packet from a player who rides nothing is ignored silently.

`isEntityCollidingWithAnythingNew(level, e, oldBox, x, y, z)` (3740–3792): `newBox =
e.getBoundingBox().move(x - e.getX(), y - e.getY(), z - e.getZ())` (the box after the
check's move, shifted to the client's position); `shapes = level.getPreMoveCollisions(e,
newBox.deflate(9.999999747378752E-6), oldBox.getBottomCenter())`; `old =
Shapes.create(oldBox.deflate(9.999999747378752E-6))`; true if any shape `s` has
`!Shapes.joinIsNotEmpty(s, old, BooleanOp.AND)`: a collision at the new place that the
old box did not already overlap. `getPreMoveCollisions` is `CollisionGetter`'s, not
read.

There is **no per-tick limit on `MoveVehicle`** (unlike the player's
`receivedPositionThisTick`, 2.5): several in one tick are each checked against the
same `vehicleFirstGood`, so a vehicle can move at most about `sqrt(100 + |v|²)` from
where it was at the end of the last `tickPlayer`.

`ClientboundMoveVehiclePacket.fromEntity(e)`: the entity's position and rotations
(**Inferred**; the packet class is not in the dump; SteelMC's codec, for orientation:
`DVec3`, `y_rot`, `x_rot`).

### 2.4 `handlePaddleBoat` (4469–4489) and `handlePlayerInput` (604–630)

`handlePaddleBoat`: `v = player.getControlledVehicle()` (the vehicle the player
controls, not the root; `Entity`/`Player`, not read); if `v instanceof AbstractBoat`:
`boat.setPaddleState(packet.getLeft(), packet.getRight())`. No other check, no reply.
The paddle state is the boat's synched data (section 4.8), so the server
only relays it to other viewers.

`handlePlayerInput`: `player.setLastClientInput(packet.input())` always; then, if
`hasClientLoaded()`: `resetLastActionTime()` and `player.setShiftKeyDown(input.shift())`.
The listener does not dismount: **sneaking to dismount is the player's tick**
(`Player.rideTick`/`ServerPlayer`, where `wantsToStopRiding()` (shift down) and
`isPassenger()` lead to `stopRiding()`; **Inferred** from the game's known shape, the
classes are not in the dump). `Input` carries forward, backward, left, right, jump,
shift, sprint (the protocol crate's `player_input` flags); a minecart reads them as
`getLastClientMoveIntent()` (finding 1).

`handlePlayerCommand` (5213–~5300), as it bears on vehicles: action 4
`START_RIDING_JUMP` and 5 `STOP_RIDING_JUMP` go to `getControlledVehicle()` if it is
`PlayerRideableJumping` (horses, camels: N7); action 6 `OPEN_INVENTORY`: if
`player.getVehicle() instanceof HasCustomInventoryScreen` →
`openCustomInventoryScreen(player)`. Of N5's entities only `AbstractChestBoat`
implements it (AbstractChestBoat.txt line 2): pressing the inventory key in a chest
boat opens the boat's chest. (The action numbering is the ordinal through
`ServerGamePacketListenerImpl$1`'s switch map, not dumped; the order START_SPRINTING,
STOP_SPRINTING, STOP_SLEEPING, START_RIDING_JUMP, STOP_RIDING_JUMP, OPEN_INVENTORY,
START_FALL_FLYING matches the cases' bodies.) `player_command` is serverbound 42.

### 2.5 A riding player's own moves: `handleMovePlayer` (2913–3018) and `handlePlayerPositionChange` (3019–3574)

`handleMovePlayer`: invalid values → disconnect `invalid_player_movement`; a packet
with a position when one was already taken this tick (`receivedPositionThisTick`,
cleared by `handleClientTickEnd`, 6708–6730) → disconnect; `wonGame` → return;
`tickCount == 0` → `resetPosition()`; `!hasClientLoaded()` → return; rotations default
to the player's; if `updateAwaitingTeleport()` → only `absSnapRotationTo(wrap(yRot),
wrap(xRot))`, return; else `handlePlayerPositionChange(x, y, z, yRot, xRot, onGround,
horizontalCollision)`.

`handlePlayerPositionChange`, first branch (offsets 33–92): **if
`player.isPassenger()`**: `player.absSnapTo(player.getX(), getY(), getZ(),
wrapDegrees(yRot), wrapDegrees(xRot))` and `chunkSource.move(player)`; return. The
packet's position, on-ground and collision flags are ignored; no "moved wrongly", no
floating count, no fall. `handleClientTickEnd` sets `knownMovement` to zero when no move
was known this tick (a passenger whose vehicle it does not drive has zero known
movement).

### 2.6 `handleAcceptTeleportPacket` (1130–1217) and teleports

If `packet.id == awaitingTeleport`: if `awaitingPositionFromClient == null` or the
packet's values are invalid → disconnect `invalid_player_movement`; else
`player.absSnapTo(awaiting position, player yRot, xRot)`, `lastGood = awaiting
position`, `player.hasChangedDimension()`, `awaitingPositionFromClient = null`, and
`handlePlayerPositionChange(packet x, y, z, yRot, xRot, false, false)` (which, for a
passenger, takes only the rotations). Ids that do not match are ignored.

`teleport(pos, relatives)` (3793–3848): `awaitingTeleportTime = tickCount`;
`++awaitingTeleport` (wrapping to 0 at `Integer.MAX_VALUE`);
`player.teleportSetPosition(...)`; `awaitingPositionFromClient = player.position()`;
send `ClientboundPlayerPositionPacket` (`player_position`, clientbound 73). For
vehicles: **while a teleport is awaited every `MoveVehicle` is dropped** (2.3 step 2),
and the teleport is sent again each 20 ticks. A dismount places the player with
`dismountTo` (Entity.txt 9645) and `ServerPlayer`'s override, which **Inferred** sends
a teleport (not read): until the client confirms, its moves are dropped.
`STOP_SLEEPING` sets `awaitingPositionFromClient` without sending a teleport (5213+,
offsets 103–132).

### 2.7 `handleInteract(ServerboundInteractPacket)` (5566–5679)

1. `!hasClientLoaded()` → return.
2. `e = level.getEntityOrPart(packet.entityId())`; `player.resetLastActionTime()`;
   **`player.setShiftKeyDown(packet.usingSecondaryAction())`** (finding 3).
3. `e == null` or `!worldBorder.isWithinBounds(e.blockPosition())` → return.
4. `!player.isWithinEntityInteractionRange(e.getBoundingBox(), 3.0)` → return
   (`Player`, not read; **Inferred**: the box's squared distance from the eyes below
   `(entity_interaction_range + 3.0)²`, the range being 3, plus 2 in creative). No
   reply is sent when refused.
5. `hand = packet.hand()`, `location = packet.location()`, `stack =
   player.getItemInHand(hand)`; `!stack.isItemEnabled(features)` → return.
6. `before = stack.copy()`; `r = player.interactOn(e, hand, location)` (`Player`, not
   read; **Inferred** order, from the game's known shape: spectators open the
   spectator screen and pass; `e.interact(player, hand, location)`; if that does not
   consume the action and the stack is not empty and `e` is a `LivingEntity`,
   `stack.interactLivingEntity(player, living, hand)` (not N5's, except leads on mobs);
   creative keeps the count of a stack it used).
7. If `r instanceof InteractionResult.Success s`: the advancement trigger
   `PLAYER_INTERACTED_WITH_ENTITY` with `s.wasItemInteraction() ? before : EMPTY`
   (nothing for Clustine); if `s.shouldSwing()`: `player.swingAndResetAttackStrength(
   hand, before.getInteractAnimation(), s.swingSource() != SwingSource.PREDICTED)`:
   the swing is sent to others, and to the player itself only when the client did not
   predict it (`SUCCESS_SERVER`; the boolean, **Inferred**; `LivingEntity.swing` not
   read). `PASS`, `FAIL` and `TRY_WITH_EMPTY_HAND` do nothing here.

`InteractionResult` (InteractionResult.txt): `SUCCESS` = `Success(PREDICTED,
DEFAULT)`, `SUCCESS_SERVER` = `Success(SERVER_ONLY, DEFAULT)`, `CONSUME` =
`Success(NONE, DEFAULT)`, `FAIL`, `PASS`, `TRY_WITH_EMPTY_HAND`; `consumesAction()`
default false (true for `Success`, **Inferred**: `$Success` not dumped). What each
N5 entity returns is in its own section; mounting a boat or a cart is
`player.startRiding(this)` inside the entity's `interact` (sections 3.12, 4.6, 5.8).
`Entity.interact`'s default (Entity.txt 6475–6680) is the leash logic (section 7.4):
sneaking with mobs leashed to the player moves them onto this entity if it is
`Leashable`; shears cut its leashes; a lead on a leashable entity ties it.

The packet (**Inferred** from SteelMC's 26.3 codec, for orientation; the class is not
in the dump): varint entity id, varint hand, `location` as `LpVec3` (the hit point
relative to the entity's position, **Client**), boolean `usingSecondaryAction`.

### 2.8 `handleAttack(ServerboundAttackPacket)` (5460–5565): breaking N5's entities

1. `!hasClientLoaded()` or spectator → return. `e = getEntityOrPart(id)`;
   `resetLastActionTime()`; `e == null` or outside the world border → return.
2. `!player.isWithinAttackRange(mainHand, e.getBoundingBox(), 3.0)` → return.
3. The main hand has `DataComponents.PIERCING_WEAPON` (a spear) → return.
4. `e` is an `ItemEntity`, an `ExperienceOrb`, the player itself, or an
   `AbstractArrow` that is not `isAttackable()` → disconnect
   `multiplayer.disconnect.invalid_entity_attacked`.
5. Main hand not enabled → return; `player.cannotAttackWithItem(mainHand, 5)` →
   return.
6. `player.attack(e)` (`Player`, not read): for N5's entities this leads to
   `skipAttackInteraction`/`hurtOrSimulate` and their `hurtServer` (the two hits of an
   armour stand, a creative player's instant break of a vehicle, a hanging entity
   dropped: each entity's section).

### 2.9 `handleUseItemOn` (4022–4316), `handleUseItem` (4317–4417), pick-item

The placing items of N5 come through the paths the repo already has for blocks:
`handleUseItemOn` (ack the sequence; item enabled; `isWithinBlockInteractionRange(pos,
1.0)`; the hit location within `1.0000001` of the block's centre on each axis; build
height; spawn protection; `awaitingPositionFromClient == null` and
`level.mayInteract`) → `ServerPlayerGameMode.useItemOn` → the block's `useItemOn`/
`useWithoutItem` and then `ItemStack.useOn` → `MinecartItem.useOn`,
`ArmorStandItem.useOn`, `HangingEntityItem.useOn` (`ItemFrameItem`), `LeadItem.useOn`
(those sections). `handleUseItem` (rotations from the packet; `ServerPlayerGameMode.
useItem` → `Item.use`) → `BoatItem.use` (the ray of section 4.1). Both send
`BlockUpdate` of the clicked block and its neighbour on the face when refused.
`ServerboundPickItemFromEntityPacket` (`pick_item_from_entity`, 37; 1708–1762): within
interaction range 3.0 → `e.getPickResult()` → `tryPickItem` (1763–1814: find the stack
in the inventory, select it, or in creative `addAndPickItem`; `SetHeldSlot` and the
inventory's changes): a creative player's middle click on a minecart, boat, armour
stand, frame or painting gives its item. Not yet decoded by the protocol crate.

### 2.10 Packets of this section

| Packet | Dir | Id | In `clustine-protocol` |
| --- | --- | --- | --- |
| `accept_teleportation` | S | 0 | yes (`ConfirmTeleportation`) |
| `attack` | S | 1 | id only, not decoded |
| `client_tick_end` | S | 13 | yes |
| `interact` | S | 26 | id only, not decoded |
| `move_player_pos` / `_pos_rot` / `_rot` / `_status_only` | S | 30–33 | yes |
| `move_vehicle` | S | 34 | id only, not decoded |
| `paddle_boat` | S | 35 | id only, not decoded |
| `pick_item_from_entity` | S | 37 | no |
| `player_command` | S | 42 | no |
| `player_input` | S | 43 | yes |
| `use_item_on` / `use_item` | S | 66 / 67 | yes |
| `add_entity` | C | 1 | yes (`SpawnEntity`) |
| `animate` | C | 2 | yes |
| `block_update` | C | 8 | yes |
| `move_vehicle` | C | 58 | id only, not encoded |
| `open_screen` | C | 60 | no |
| `player_position` | C | 73 | yes (`SynchronizePlayerPosition`) |
| `set_entity_data` | C | 101 | yes (three serializers only) |
| `set_entity_link` | C | 102 | no |
| `set_entity_motion` | C | 103 | yes |
| `set_passengers` | C | 109 | no |
| `sound_entity` | C | 118 | no |
| `swing_animation` | C | 123 | no (whether a swing is `animate` or this one in 26.3 is in `LivingEntity.swing`, not read) |

The edge today: `services/edge/src/play.rs:109-203` handles the packets marked yes and
passes `Unhandled` over, so `Interact`, `Attack`, `MoveVehicle`, `PaddleBoat` and
`PlayerCommand` are dropped silently now.

---------------------------------------------------------------------------------------

## 3. Minecarts

Classes read: `MinecartItem`, `AbstractMinecart`, `MinecartBehavior`, `OldMinecartBehavior`
(and `$1`), `NewMinecartBehavior` (summarised), `Minecart`, `VehicleEntity` (data only),
`DetectorRailBlock`, `BaseRailBlock` (`isRail`), `RailShape`, and the parts of `Entity`
the cart calls (`push`, `applyGravity`, `computeSpeed`, `checkBelowWorld`, `lavaHurt`,
`lavaIgnite`, `getDirection`). Sections 3.1 to 3.13 are placing and movement; 3.14 to 3.19 are `VehicleEntity`'s
damage and breaking (shared with boats), the containers and the kinds of cart (also
`MinecartFurnace`, `MinecartChest`, `AbstractMinecartContainer`, `ContainerEntity`,
`MinecartTNT`, `MinecartHopper`, `MinecartSpawner`, `MinecartCommandBlock`).

### 3.0 Findings of this section (1 to 8 movement, 9 to 17 damage and kinds)

1. **The default behaviour is `OldMinecartBehavior`.** The constructor
   (AbstractMinecart.txt lines 23–51) takes `NewMinecartBehavior` only when
   `useExperimentalMovement(level)` (lines 1406–1412), which is
   `level.enabledFeatures().contains(FeatureFlags.MINECART_IMPROVEMENTS)`. In 26.3 that
   flag comes only with the experimental data pack `minecart_improvements`
   (`generated/data/minecraft/datapacks/minecart_improvements/`), not enabled in a new
   world. So N5 ports the old behaviour; the new one is summarised in 3.9 so that the
   plan can decide to refuse the experiment or leave it out. The choice is made per
   cart at construction and again at every use of `useExperimentalMovement` in
   `AbstractMinecart` (`move`, `applyEffectsFromBlocks`, `getCurrentBlockPosOrRailBelow`,
   `createInterpolationHandler`, `pushOtherMinecart`): a world's flag never changes
   while it runs, so one answer per world is enough.
2. **`AbstractMinecart.tick` does not call `super.tick()`, so no `Entity.baseTick`**
   (lines 486–536): no fire-tick countdown and no on-fire flag, no freezing, no eyes in
   fluid. It runs its own short list (3.3). **Inferred**: a cart in lava is set on
   fire (`lavaIgnite`) but its remaining fire ticks never count down and the on-fire
   shared flag is never set by it; what is shown is the client's.
3. **A rideable cart moving at 0.1 a tick or more picks up any pushable entity that is
   not a player, an iron golem or a minecart** (`OldMinecartBehavior.
   pushAndPickupEntities`, lines 1347–1451): `e.startRiding(cart)`. Among N5's
   entities that is a **boat** (boats are pushable: the game's boat-in-a-minecart) and
   any other pushable N5 entity (**Inferred**: armour stands are not pushable when
   markers; to check in their section). Mobs are N7. The plan needs a rule for a
   vehicle (a boat) riding a vehicle (the cart): ADR-0028's unit of root and
   passengers covers it, the root being the cart.
4. **A cart's own move collides with other entities' boxes**: `canCollideWith(e)` is
   `AbstractBoat.canVehicleCollide(this, e)` (line 159), `(e.canBeCollidedWith(this) ||
   e.isPushable()) && !isPassengerOfSameVehicle(e)`, so in `Entity.collide` the cart is
   stopped by players, boats, other carts and pushable mobs as by blocks (**Inferred**
   from `CollisionGetter.getEntityCollisions`, not read). N4's collision reads only
   blocks; N5 needs entity colliders for carts and boats, and they read **players'
   boxes**, which are the client's (ADR-0005), so a cart's path depends on where the
   edge says a player stands in the tick.
5. **The rider's input reaches the cart through `ServerPlayer.getLastClientMoveIntent()`**
   (moveAlongTrack, OldMinecartBehavior.txt lines 449–500), not through
   `ServerboundPlayerInputPacket` directly; `handlePlayerInput` only stores the input
   (`setLastClientInput`). How the intent is derived is in `ServerPlayer` (not read).
6. **Under the old behaviour a cart moves at most 0.4 a tick on each horizontal axis**
   (on rails, off rails, in water 0.2) plus the snap to its rail: it never crosses more
   than one block a tick, so never two chunks (section 10). Under the new behaviour
   the speed is the game rule `max_minecart_speed` / 20 per tick (3.9), up to many
   blocks a tick along rails: a reason to keep the experiment out.
7. **Detector rails need the entity half that the repo lacks** (3.8): `rails.rs` has
   no `entityInside`, scheduled tick, `checkPressed`, own signal (15 while powered,
   direct 15 upwards only) or analogue output; `clustine_blocks::signal::own_signal`
   gives a detector rail 0.
8. **`MinecartItem.useOn` spawns with `EntitySpawnReason.DISPENSER`** (MinecartItem.txt
   line 87) even for a player's use. Harmless (the reason only reaches
   `EntityType.create`), but name it so that nobody "fixes" it.
9. **A container vehicle drops its contents every time it is removed as destroyed, in
   creative too and whatever `doEntityDrops` says.** `AbstractMinecartContainer.remove`
   (lines 83–100) and `AbstractChestBoat.remove` (lines 69–86) call
   `Containers.dropContents` for any `RemovalReason` whose `shouldDestroy()` is true, and
   that is `KILLED` and `DISCARDED` (`Entity$RemovalReason.txt` static init: the first
   boolean is `destroy`). A creative break `discard()`s the vehicle (no vehicle item) but
   the contents still fall out. A survival break `kill()`s it first (`remove(KILLED)`
   drops the contents), and only then reads `ENTITY_DROPS` for the vehicle's own item;
   `chestVehicleDestroyed` reads the rule a second time and calls `dropContents` again on
   what is by then an empty container (**Inferred**: `Containers.dropContents` empties the
   stacks it drops, by `split`; not read). So the game rule keeps only the vehicle's item
   back.
10. **A creative player's hit discards; it does not kill.** No item, no `ENTITY_DIE`
    game event; the damage is still added and sent first. The one way past this is
    `shouldSourceDestroy`, true only for a TNT minecart hit by fire, an explosion or a
    burning projectile.
11. **`damage`, `hurt time` and `hurt direction` are synched data and are not saved.**
    They decay by one per tick in the vehicle's tick (`AbstractMinecart.tick` lines
    486–536; boats in theirs). The game never saves them, but a cart crossing a line is
    not saved in the game either: an arrival should carry the three, or a cart punched
    four times just before a line is whole again after it (a seam a survival player
    would find; creative breaks in one hit).
12. **A chest or hopper minecart's rolling drag depends on what it holds** (unless it
    still has a loot table): `AbstractMinecartContainer.applyNaturalSlowdown`
    (lines 129–162) replaces the behaviour's factor by `0.98f + (15 − comparator
    signal) · 0.001f`. A full chest cart stops sooner than an empty one. N5's sim needs
    the contents as items, with their maximum stack sizes, not only as bytes, and
    `AbstractContainerMenu.getRedstoneSignalFromContainer` (not read).
13. **A TNT minecart explodes on its own** (a horizontal collision at
    `horizontalDistanceSqr ≥ 0.01`, a fall of 3 or more, a powered activator rail, being
    broken while moving), and a **hopper minecart takes in items** around it every tick.
    Both items can be placed by `MinecartItem`. The plan has to choose: leave the two
    items unplaceable until N6 (explosions, hoppers), or place them inert and name the
    difference. The spawner minecart has no item; the command block minecart is given
    only by commands.
14. **A powered activator rail under a plain minecart ejects its riders every tick and
    sets its damage to 50 every ten ticks** (`Minecart.activateMinecart`, lines 55–80):
    above the 40 that breaks it, though the setting breaks nothing; one survival hit
    (`+10`) then does.
15. **The furnace minecart's random is drawn every tick it has fuel, on the server**
    (`nextInt(4)`, for a smoke particle that `ServerLevel` does not send), and its push
    is not normalised when it is fuelled: its length is the player's horizontal distance
    from the cart. Its interaction answers `SUCCESS` with any item in hand.
16. Container minecarts start with **36** slots in their list (`AbstractMinecartContainer`
    constructor, lines 9–20) though a chest cart's size is 27 and a hopper cart's 5; only
    loading (`clearItemStacks`, lines 234–242) resizes it. Nothing fills the slots beyond
    the size, so this matters only to a porter who sizes the list by the constructor.
17. Spawner and command block minecarts drop a **plain minecart** (`getDropItem`); the
    furnace, chest, TNT and hopper carts drop their own items; boats drop the item their
    type was made with (`AbstractBoat.getDropItem`, final, line 2237: section 4.8).

### 3.1 `MinecartItem.useOn(ctx)` (MinecartItem.txt lines 15–147)

1. `state = level.getBlockState(clickedPos)`; if `!state.is(#minecraft:rails)` →
   `InteractionResult.FAIL` (the tag, not the class).
2. `shape = state.getBlock() instanceof BaseRailBlock ? state.getValue(shapeProperty)
   : NORTH_SOUTH`. (`shapeProperty` is `RailBlock.SHAPE` for the plain rail, the
   straight-only property for powered, detector and activator rails.)
3. `dy = shape.isSlope() ? 0.5 : 0.0` (`isSlope`: the four `ASCENDING_*`,
   RailShape.txt lines 65–80).
4. `p = (x + 0.5, y + 0.0625 + dy, z + 0.5)` (doubles; `y + 0.0625` then `+ dy`).
5. `cart = AbstractMinecart.createMinecart(level, p.x, p.y, p.z, type,
   EntitySpawnReason.DISPENSER, stack, player)` (3.2); null → `FAIL`.
6. Only with the experiment: if any entity in the cart's box is an `AbstractMinecart`
   → `FAIL` (lines 98–121). Under the old behaviour carts may be placed into each
   other.
7. On the server: `addFreshEntity(cart)`; `gameEvent(ENTITY_PLACE, clickedPos,
   Context.of(player, level.getBlockState(clickedPos.below())))`.
8. `stack.shrink(1)` (on both sides), `SUCCESS`. In creative the stack is given its
   count back by the caller (`ServerPlayerGameMode.useItemOn` keeps the count of an
   infinite-materials player: **Inferred**, not read here).

The dispenser's minecart behaviour (`MinecartDispenseItemBehavior`) is N6.

### 3.2 `AbstractMinecart.createMinecart` and construction (lines 23–128)

- Constructor: `VehicleEntity(type, level)`; `flipped = false`; `blocksBuilding = true`
  (a block cannot be placed into its box); `behavior` as in finding 1.
- `createMinecart(level, x, y, z, type, reason, stack, player)`: `cart =
  type.create(level, reason)`; if non-null: `setInitialPos(x, y, z)` (`setPos`, then
  `xo/yo/zo` = the same); `EntityType.createDefaultStackConfig(level, stack,
  player).apply(cart)` (the stack's custom name and entity data components: **Not
  read**); only with the experiment, `adjustToRails(getCurrentBlockPosOrRailBelow(),
  its state, true)`. No rotation is set: a placed cart has `yRot = 0` (**Inferred**:
  the constructor's default) whichever way the player faces.
- `getMovementEmission()` = `EVENTS` (game events, no step sounds). The rolling sound is
  the client's (**Client**).
- `getDefaultGravity()` (lines 476–485) = `isInWater() ? 0.005 : 0.04`.
- `getAirDrag()` = `0.95f` (lines 759–763).
- `isPushable()` = true; `isPickable()` = `!isRemoved()`; `isRideable()` = false
  (`Minecart` overrides: true); `isFurnace()` = false (`MinecartFurnace`: true).
- `getBlockSpeedFactor()` (lines 412–429): `1.0f` if the block at `blockPosition()` is in
  `#minecraft:rails`, else `Entity`'s (n4-brief 1.8).
- `animateHurt(f)` (client only, lines 430–450): the shake.
- `getPassengerAttachmentPoint(e, dims, scale)` (lines 180–202): `(0, 0, 0)`
  (`LOWERED_PASSENGER_ATTACHMENT`) for a `Villager` or `WanderingTrader`; else
  `VehicleEntity`'s (section 1, the type's attachments).

### 3.3 `AbstractMinecart.tick()` (lines 486–536), in order

1. `if hurtTime > 0: hurtTime -= 1` (data 8, so dirty every tick while shaking).
2. `if damage > 0f: damage -= 1f` (data 10).
3. `checkBelowWorld()`: `getY() < level.getMinY() - 64` → `onBelowWorld()` →
   `discard()` (Entity.txt 1144–1158).
4. `computeSpeed()`: `lastKnownSpeed = position - lastKnownPosition` (first time:
   `lastKnownPosition = position`), then `lastKnownPosition = position` (Entity.txt
   1107–1127). (Read by `getKnownMovement`, below.)
5. `handlePortal()`: nether portals; none in N5 (name it).
6. `behavior.tick()` (3.4).
7. `updateFluidInteraction()` (n4-brief section 4: `EntityFluidInteraction.update` and
   the current's push). Note the order: the behaviour's tick reads `isInWater()` as last
   tick's fluid update left it.
8. If `isInLava()`: `lavaIgnite()` (`igniteForSeconds(15f)` unless fire immune);
   `lavaHurt()` (unless fire immune: `hurtServer(level, lava, 4.0f)`, and if that
   returns true and `shouldPlayLavaHurtSound()` and not silent: sound
   `generic.burn`, volume `0.4f`, pitch `2.0f + random.nextFloat() * 0.4f`, the
   entity's random (Entity.txt 1215–1264)); `fallDistance *= 0.5`.
9. `firstTick = false`.

`Minecart.tick()` (Minecart.txt lines 87–130) wraps it; what it adds (`rotationOffset`)
is the client's only.

### 3.4 `OldMinecartBehavior.tick()` (OldMinecartBehavior.txt lines 42–222)

- **Client** (lines 54–74): if the interpolation is not active, `reapplyPosition()` and
  `xRot %= 360f`, `yRot %= 360f`. The client runs no physics; it interpolates what the
  server sends (`LinearInterpolationHandler`, `createInterpolationHandler`, lines
  659–670; `lerpMotion` sets `targetDeltaMovement` and the motion, lines 31–40;
  `onInterpolationStart` sets the motion to `targetDeltaMovement`, lines 23–29).
- **Server**:
  1. `minecart.applyGravity()`: `motion.y -= getGravity()` when not 0 (`getGravity()` =
     0 when `NoGravity`, else 3.2's), Entity.txt 3924–3942.
  2. `pos = getCurrentBlockPosOrRailBelow()` (3.5); `state = level.getBlockState(pos)`.
  3. `onRails = BaseRailBlock.isRail(state)` (in `#minecraft:rails` **and** a
     `BaseRailBlock`, BaseRailBlock.txt 19–32); `setOnRails(onRails)`.
  4. If on rails: `moveAlongTrack(level)` (3.6); then if `state.is(ACTIVATOR_RAIL)`:
     `minecart.activateMinecart(level, pos.x, pos.y, pos.z, state.getValue(POWERED))`
     **every tick** it is on one, powered or not (3.7).
     Else: `minecart.comeOffTrack(level)` (3.5).
  5. `minecart.applyEffectsFromBlocks()`: under the old behaviour,
     `applyEffectsFromBlocks(position(), position())` (one `Movement(p, p)`, Entity.txt
     2282–2292; n4-brief 1.9) and `clearMovementThisTick()` (AbstractMinecart.txt
     818–836). So a cart touches only the blocks of its box where it ends, not the
     blocks along its path. `AbstractMinecart.move` under the old behaviour also calls
     `applyEffectsFromBlocks()` after `super.move` (lines 774–817): each tick's move
     (one, in `moveAlongTrack` or `comeOffTrack`) is followed by it, so a moving cart
     applies the blocks it is in **twice a tick** (at the end of its move, and here at
     the same position unless a snap moved it). The detector rail's `entityInside`
     (3.8) is idempotent within a tick.
  6. `xRot = 0f`.
  7. Yaw: `dx = xo - x`, `dz = zo - z` (old minus new, so the cart "faces" backwards
     along its motion); if `dx*dx + dz*dz > 0.001`: `yRot = (float)(Mth.atan2(dz, dx) *
     180.0 / 3.141592653589793)`, then if `flipped`, `yRot += 180f`.
  8. `delta = (double) Mth.wrapDegrees(yRot - yRotO)`; if `delta < -170.0 || delta >=
     170.0`: `yRot += 180f`; `flipped = !flipped`. (`yRotO` is the yaw at the start of
     the tick, `setOldPosAndRot` in `commonTick`.)
  9. `xRot %= 360f`; `yRot %= 360f` (Java `frem`: sign of the dividend, Rust `%` on
     `f32`).
  10. `pushAndPickupEntities()` (3.10); its result is not used.

The yaw is what `getMotionDirection()` reads (`flipped ? getDirection().getOpposite().
getClockWise() : getDirection().getClockWise()`, lines 1452–1468, `getDirection()` =
`Direction.fromYRot(yRot)`), used by the dismount (3.11).

### 3.5 Helpers

**`getCurrentBlockPosOrRailBelow()`** (AbstractMinecart.txt 544–607), old behaviour:
`(floor x, floor y, floor z)`; if the block at `y - 1` is in `#minecraft:rails`, `y -= 1`.
(New behaviour: tests the block at `floor(y - 0.1 - 1.0E-5f widened)` instead.)

**`getMaxSpeed(level)`** (lines 608–615 → OldMinecartBehavior.txt 1503–1512): `isInWater()
? 0.2 : 0.4` (`MAX_SPEED_IN_WATER`, `MAX_SPEED_ON_LAND`). `MinecartFurnace` overrides
it (3.17).

**`getSlowdownFactor()`** (OldMinecartBehavior.txt 1514–1523): `isVehicle() ? 0.997 :
0.96`.

**`applyNaturalSlowdown(v)`** (AbstractMinecart.txt 959–980): `f = getSlowdownFactor()`;
`r = v.multiply(f, 0.0, f)` (**y becomes 0**); if `isInWater()`: `r = r.scale(
0.949999988079071)` (`0.95f` widened). `MinecartFurnace` overrides it (3.17).

**`comeOffTrack(level)`** (lines 707–758):
1. `m = getMaxSpeed(level)`; `v = motion`; `motion = (clamp(v.x, -m, m), v.y, clamp(v.z,
   -m, m))` (`Mth.clamp(double, double, double)`).
2. If `onGround()`: `motion = motion.scale(0.5)`.
3. `move(MoverType.SELF, motion)` (n4-brief 1.2, with entity colliders, finding 4; then
   `applyEffectsFromBlocks()`, 3.4 step 5).
4. If not `onGround()` (after the move): `motion = motion.scale((double) 0.95f)`
   (`getAirDrag()` widened: `0.949999988079071`).

So off rails a cart on the ground loses half its speed each tick, in the air 5 %; it
falls with gravity 0.04 (0.005 in water), with no cap on `y`.

**`getRedstoneDirection(pos)`** (lines 863–946), new behaviour only; the old one has
the same rule inline (3.6 step 13). **`isRedstoneConductor(pos)`** (947–958):
`level.getBlockState(pos).isRedstoneConductor(level, pos)` (the repo's
`clustine_blocks::connect` conductor rule).

**`getKnownMovement()`** (650–658 → OldMinecartBehavior.txt 1470–1501): `Entity`'s known
movement (`lastKnownSpeed` for non-players, **Inferred**) with any NaN → `ZERO`, else
`(clamp(x, -0.4, 0.4), y, clamp(z, -0.4, 0.4))`. Read by others (for example a
rider's or a projectile's inheritance; **Not read** who).

**`exits(shape)`** (lines 461–468; the table built in `lambda$static$0`, lines
1435–1502). With `W = (-1,0,0)`, `E = (1,0,0)`, `N = (0,0,-1)`, `S = (0,0,1)` and `X↓` =
`X` with `y - 1`, `(first, second)`:

| shape | first | second |
|---|---|---|
| `NORTH_SOUTH` | N | S |
| `EAST_WEST` | W | E |
| `ASCENDING_EAST` | W↓ | E |
| `ASCENDING_WEST` | W | E↓ |
| `ASCENDING_NORTH` | N | S↓ |
| `ASCENDING_SOUTH` | N↓ | S |
| `SOUTH_EAST` | S | E |
| `SOUTH_WEST` | S | W |
| `NORTH_WEST` | N | W |
| `NORTH_EAST` | N | E |

(`ASCENDING_EAST` rises towards the east: its west end is the low one.) `RailShape`'s
ordinals are the order of this table (RailShape.txt lines 3–21, 140–205).

### 3.6 `OldMinecartBehavior.moveAlongTrack(level)` (lines 223–952)

Variables are named as in the bytecode's locals where it helps. `pos` =
`getCurrentBlockPosOrRailBelow()`, `state` = its block (looked up again; the same as the
tick's).

1. `minecart.resetFallDistance()`.
2. `x, y, z` = the cart's position; `before = getPos(x, y, z)` (3.6.1, may be null).
3. `y = (double) pos.getY()` (the cart's own `y` is not used again in this method
   except through `getPos`).
4. `powered = false`, `braking = false`; if `state.is(POWERED_RAIL)`: `powered =
   state.getValue(POWERED)`, `braking = !powered`.
5. `slope = 0.0078125` (`1/128`); if `isInWater()`: `slope *= 0.2`.
6. `v = motion`; `shape = state.getValue(shapeProperty)`.
7. **Slopes** (the `$1` switch map, OldMinecartBehavior$1.txt; tableswitch at
   lines 304–360):
   - `ASCENDING_EAST`: `motion = v + (-slope, 0, 0)`; `y += 1.0`;
   - `ASCENDING_WEST`: `motion = v + (slope, 0, 0)`; `y += 1.0`;
   - `ASCENDING_NORTH`: `motion = v + (0, 0, slope)`; `y += 1.0`;
   - `ASCENDING_SOUTH`: `motion = v + (0, 0, -slope)`; `y += 1.0`;
   - others: nothing.
   (Always downhill: east-rising pushes west, and so on.)
8. `v = motion`; `(first, second) = exits(shape)`; `ex = second.x - first.x`, `ez =
   second.z - first.z` (ints widened); `len = Math.sqrt(ex*ex + ez*ez)`.
9. **Direction along the rail**: `dot = v.x*ex + v.z*ez`; if `dot < 0`: `ex = -ex`, `ez =
   -ez`. `h = Math.min(2.0, v.horizontalDistance())`; `v = (h*ex/len, v.y, h*ez/len)`
   (as written: `h * ex / len`, left to right); `motion = v`. So the whole horizontal
   speed (up to 2.0) is turned onto the rail's line, keeping its sense; at a curve the
   line is the diagonal between its two exits. A cart at rest (`dot == 0`) keeps the
   exits' sense with `h = 0`: still at rest.
10. **The rider's push** (lines 441–500): `p = getFirstPassenger()`; `intent = (p is a
    ServerPlayer) ? p.getLastClientMoveIntent() : ZERO` (the first passenger again,
    read twice); if `p instanceof Player` and `intent.lengthSqr() > 0`: `n =
    intent.normalize()` (only its `lengthSqr() > 0` is tested, it is not used
    otherwise); if `n.lengthSqr() > 0` and `motion.horizontalDistanceSqr() < 0.01`:
    `motion = motion + (intent.x * 0.001, 0, intent.z * 0.001)` (the **unnormalised**
    intent) and `braking = false`. So a rider can start a cart that moves at under 0.1
    a tick, even on an unpowered powered rail, and cannot speed up a faster one.
11. **Braking** (an unpowered powered rail, no rider's push): `h =
    motion.horizontalDistance()`; if `h < 0.03`: `motion = ZERO`; else `motion =
    motion.multiply(0.5, 0.0, 0.5)` (**y becomes 0**).
12. **Snap onto the rail's line** (lines 523–636): `x0 = pos.x + 0.5 + first.x * 0.5`, `z0
    = pos.z + 0.5 + first.z * 0.5`, `x1 = pos.x + 0.5 + second.x * 0.5`, `z1 = pos.z +
    0.5 + second.z * 0.5` (ints widened; `+ 0.5` then `+ first.x * 0.5`);
    `ex = x1 - x0`, `ez = z1 - z0` (**these overwrite step 8's `ex`, `ez`**);
    `t = (ex == 0.0) ? z - pos.z : (ez == 0.0) ? x - pos.x : ((x - x0)*ex + (z - z0)*ez)
    * 2.0` (`x`, `z` the cart's position from step 2); `x = x0 + ex*t`, `z = z0 + ez*t`;
    `setPos(x, y, z)` with step 3/7's `y`. (On a straight rail `t` is just the
    position's offset in the block; on a curve, twice the projection.)
13. **The move**: `k = isVehicle() ? 0.75 : 1.0`; `m = minecart.getMaxSpeed(level)`
    (the furnace cart's override counts here); `v = motion`;
    `minecart.move(SELF, (clamp(k*v.x, -m, m), 0.0, clamp(k*v.z, -m, m)))`. A
    vertical move is never asked for here: the slope's height comes from the snaps.
14. **Climbing to the next block of a slope** (lines 676–752): if `first.y != 0` and
    `floor(x') - pos.x == first.x` and `floor(z') - pos.z == first.z` (`x', z'` the
    position after the move): `setPos(x', y' + first.y, z')`; else if `second.y != 0`
    and the same test with `second`: `setPos(x', y' + second.y, z')`. (`first.y`,
    `second.y` are 0 or -1: this lowers a cart that left a slope by its low end.)
15. `motion = minecart.applyNaturalSlowdown(motion)` (3.5; y becomes 0; the furnace's
    override counts).
16. **Speed from height** (lines 770–818): `after = getPos(x, y, z)` at the current
    position; if both `after` and `before` (step 2) are non-null: `d = (before.y -
    after.y) * 0.05`; `v = motion`; `h = v.horizontalDistance()`; if `h > 0`: `motion =
    v.multiply((h + d)/h, 1.0, (h + d)/h)`; then `setPos(x, after.y, z)` (whether or not
    `h > 0`). So a cart gains 0.05 of the height it lost in the tick, and loses it
    going up.
17. **Turning into the new block** (lines 819–860): `bx = floor(x)`, `bz = floor(z)`; if
    `bx != pos.x || bz != pos.z`: `v = motion`; `h = v.horizontalDistance()`; `motion =
    (h * (bx - pos.x), v.y, h * (bz - pos.z))` (ints widened). The whole speed goes
    along the axis of the block entered (both axes, by `h` each, if it entered
    diagonally).
18. **Powered rail push** (lines 861–951), if `powered`: `v = motion`; `h =
    v.horizontalDistance()`; if `h > 0.01`: `motion = v + (v.x/h * 0.06, 0, v.z/h *
    0.06)` (as written: `v.x / h * 0.06`). Else (starting from rest): `sx = v.x`, `sz =
    v.z`; if `shape == EAST_WEST`: if the block west of `pos` is a conductor, `sx =
    0.02`; else if the block east is, `sx = -0.02`; else if `shape == NORTH_SOUTH`: if
    north is a conductor, `sz = 0.02`; else if south is, `sz = -0.02`; **any other shape
    (a sloped powered rail): return with no change**; `motion = (sx, v.y, sz)`. (A cart
    at rest is kicked away from a solid block at one end.)

Nothing in it draws a random number.

#### 3.6.1 `getPos(x, y, z)` (lines 1121–1341)

`bx, by, bz = floor(x), floor(y), floor(z)`; if the block at `(bx, by - 1, bz)` is in
`#minecraft:rails`: `by -= 1`. `state = block at (bx, by, bz)`; if not
`BaseRailBlock.isRail(state)`: **null**. Else `(first, second) = exits(shape)`;
`x0 = bx + 0.5 + first.x*0.5`, `y0 = by + 0.0625 + first.y*0.5`, `z0 = bz + 0.5 +
first.z*0.5`; `x1, y1, z1` likewise with `second`; `ex = x1 - x0`, `ey = (y1 - y0) *
2.0`, `ez = z1 - z0`; `t = (ex == 0.0) ? z - bz : (ez == 0.0) ? x - bx : ((x - x0)*ex +
(z - z0)*ez) * 2.0`; `x = x0 + ex*t`, `y = y0 + ey*t`, `z = z0 + ez*t`; if `ey < 0`: `y
+= 1.0`; else if `ey > 0`: `y += 0.5`. Return `(x, y, z)`. (The `+ 0.0625` is the rail's
height, the same as `MinecartItem`'s.)

`getPosOffs(x, y, z, d)` (lines 953–1120) is the same walked `d` along the rail; only the
client's renderer uses it (**Client**). `stepAlongTrack` returns 0 (lines 1342–1345).

### 3.7 Activator rails: `activateMinecart(level, x, y, z, powered)`

- `AbstractMinecart`: nothing (lines 616–619).
- `Minecart` (the rideable cart, Minecart.txt lines 55–80): if `powered`: if
  `isVehicle()`, `ejectPassengers()` (each passenger `stopRiding`, section 1, with the
  dismount of 3.11); then if `hurtTime == 0`: `hurtDir = -hurtDir`, `hurtTime = 10`,
  `damage = 50f` (set, not through `hurtServer`, so it never breaks the cart),
  `markHurt()` (the motion is sent: `hurtMarked`). It fires every tick the cart is on
  a powered activator rail; the shake restarts once `hurtTime` has run down.
- Named only: `MinecartTNT` (primes when powered, N6), `MinecartHopper` (`setEnabled(
  !powered)`, N6), `MinecartCommandBlock` (runs its command when powered, every 4
  ticks, N6 or never). Chest, furnace and spawner carts do nothing.

### 3.8 Detector rails (DetectorRailBlock.txt)

The cart's part is only that its box is inside the rail's block when the cart's block
effects run (3.4 step 5). The block's rules, for the porter of the rail's half:

- `entityInside(state, level, pos, entity, …)` (lines 55–74), server only: if not
  `POWERED`, `checkPressed(level, pos, state)`. **Any entity** inside triggers it, but
  `checkPressed` looks only for carts.
- `tick(state, level, pos, random)` (the scheduled tick, 75–90): if `POWERED`,
  `checkPressed`. No draw.
- `checkPressed(level, pos, state)` (109–228): return unless `canSurvive(state, level,
  pos)` and the block at `pos` is still this block. `was = POWERED`; `now = any
  AbstractMinecart in getSearchBB(pos)` (`getEntitiesOfClass`, predicate always true);
  `getSearchBB(pos)` (353–395) = `AABB(x + 0.2, y, z + 0.2, x + 1 - 0.2, y + 1 - 0.2, z +
  1 - 0.2)` (ints widened, then the arithmetic in that order). If `now && !was`: set
  `POWERED = true` (`setBlockAndUpdate`), `updatePowerToConnected(level, pos, new,
  true)`, `updateNeighborsAt(pos, this)`, `updateNeighborsAt(pos.below(), this)`,
  `setBlocksDirty`. If `!now && was`: the same with `false`. Then if `now`:
  `scheduleTick(pos, this, 20)`. Always: `updateNeighbourForOutputSignal(pos, this)`
  (comparators).
- `updatePowerToConnected` (229–265): for each position of `new RailState(level, pos,
  state).getConnections()`: `level.neighborChanged(state there, that pos, its block,
  null, false)` (so powered rails along its line re-read power). The repo has
  `RailState` in `rails.rs` (`Rail`).
- `onPlace` (266–287): if the old state was not this block: `updateState(...)` (the
  rail's own, in `rails.rs`) then `checkPressed` with the state it returned.
- Signals: `isSignalSource` true; `ownSignal` = `POWERED ? 15 : 0` (42–53);
  `getDirectSignal(…, dir)` = `POWERED && dir == UP ? 15 : 0` (91–108) (strong power to
  the block below only).
- `getAnalogOutputSignal` (298–341): 0 unless `POWERED`; else the first
  `MinecartCommandBlock` in the search box gives its success count; else the first
  cart in the box matching `EntitySelector.CONTAINER_ENTITY_SELECTOR` (a container
  cart) gives `AbstractContainerMenu.getRedstoneSignalFromContainer(cart)` (**Not
  read**: the comparator formula over the cart's slots; the repo has it for block
  containers, `block_entities.rs`); else 0. The first in `getEntitiesOfClass` order
  (**Inferred**: section then insertion order) when several carts are there.
- Property `SHAPE` is `RAIL_SHAPE_STRAIGHT`, plus `POWERED`, `WATERLOGGED` (465–471).

**What `rails.rs` covers** (crates/clustine-sim/src/region/rails.rs): placing and
joining (`onPlace`, `updateState`, `updateDir`, `RailState`), the survival rule
(`neighborChanged`, `shouldBeRemoved`), powered and activator rails' power along the
line (`PoweredRailBlock.updateState`, `findPoweredRailSignal`) and the removal's
updates. **Missing for N5**: the detector rail's `entityInside`, scheduled tick,
`checkPressed`, `updatePowerToConnected` and its own `onPlace`; its signals
(`clustine_blocks::signal::own_signal` and `block_direct_signal` know no detector
rail; `connect.rs` already counts it a signal source) and its analogue output
(`signal::analog_output` has none; it needs the carts in the box, so it is the
region's, not the block crate's).

### 3.9 `NewMinecartBehavior` (behind the experiment; summary)

For the plan to decide; nothing of it is in a default 26.3 world.

- Tick (NewMinecartBehavior.txt 51–109): on the client it lerps through the steps the
  server sent (`lerpClientPositionAndRotation`, `POS_ROT_LERP_TICKS = 3`); on the
  server, on its first tick `adjustToRails(pos, state, true)` (429–733: snaps to the
  rail with `ON_RAIL_Y_OFFSET = 0.1` and sets yaw and pitch from the rail), then
  gravity, then `moveAlongTrack`.
- `moveAlongTrack` (786–1069) iterates (`TrackIteration`): while `movementLeft` and the
  cart is alive, on rails it activates an activator rail, computes the speed
  (`calculateTrackSpeed`, 1070–1177: slope gain `max(0.0078125, h*0.02)` (×0.2 in
  water), the rider's push (normalised intent × 0.001 under 0.1 a tick), halting on an
  unpowered powered rail (`< 0.03` → 0, else × 0.5), the natural slowdown with speed
  capped at `getMaxSpeed`, the boost (× (1 + 0.06/len) above 0.01, else 0.2 away
  from a conductor), each once per tick) and walks it along the track block by block
  (`stepAlongTrack`, 1364–1599, with `restAtVShape` and
  `OPPOSING_SLOPES_REST_AT_SPEED_THRESHOLD = 0.005`), recording `MinecartStep`s for
  the client; off rails, `comeOffTrack`. Yaw from the movement, pitch from the slope
  (`setRotation`).
- `getMaxSpeed` (1631–1650) = `gameRules.get(MAX_MINECART_SPEED) * (isInWater() ? 0.5
  : 1.0) / 20.0` (blocks a tick; the rule's default is **Not read**); `getSlowdownFactor`
  = `isVehicle() ? 0.997 : 0.975`.
- Pushing and picking up (`pushAndPickupEntities`, `pickupEntities`, `pushEntities`,
  1715–1906) after its move (`AbstractMinecart.move`, which also moves the cart back by
  what a push undid); `pushOtherMinecart` uses the cart's own motion and no 0.8 test.
- Sent with `ClientboundMoveMinecartPacket` of the steps (`ServerEntity.
  handleMinecartPosRot`, ServerEntity.txt 712–), not the usual move packets; no
  interpolation handler on the client (`NO_OP`).
- `MinecartItem` refuses a cart into another cart's box.

**Recommendation (Inferred)**: refuse or ignore the `minecart_improvements` experiment
in N5; its speed reads rails many blocks ahead in one tick and its client packet is
another codec.

### 3.10 Pushing and being pushed

**`OldMinecartBehavior.pushAndPickupEntities()`** (lines 1347–1451), at the end of every
server tick:
- `box = cart.getBoundingBox().inflate(0.20000000298023224, 0.0, 0.20000000298023224)`
  (`0.2f` widened).
- If `cart.isRideable()` (the plain `Minecart`) and `motion.horizontalDistanceSqr() >=
  0.01`: `list = level.getEntities(cart, box, EntitySelector.pushableBy(cart))`
  (**Not read**: `pushableBy`; **Inferred**: not spectators, `isPushable()`, and the
  team collision rule). For each `e`, in the list's order: if `e` is a `Player`, an
  `IronGolem` or an `AbstractMinecart`, or the cart already has a passenger
  (`isVehicle()`), or `e.isPassenger()`: `e.push(cart)`; else `e.startRiding(cart)`
  (finding 3).
- Else: for each `e` in `level.getEntities(cart, box)` (no predicate): if
  `!cart.hasPassenger(e) && e.isPushable() && e instanceof AbstractMinecart`:
  `e.push(cart)`.
- Returns false.

`e.push(cart)` runs `e`'s `push(Entity)`: for another cart, `AbstractMinecart.push`
(below); for a player or a boat, `Entity.push(Entity)` (Entity.txt 5187–5282, unless
overridden: `LivingEntity` and `AbstractBoat` are **Not read** here for it):
return if `isPassengerOfSameVehicle(other)`, or either has `noPhysics`; `dx = other.x -
x`, `dz = other.z - z`; `d = Mth.absMax(dx, dz)`; if `d >= 0.009999999776482582`
(`0.01f`): `d = sqrt(d)`; `dx /= d`, `dz /= d`; `k = min(1.0, 1.0/d)`; `dx *= k`, `dz
*= k`; `dx *= 0.05000000074505806`, `dz *= …` (`0.05f`); if `!isVehicle() &&
isPushable()`: `this.push(-dx, 0, -dz)`; if `!other.isVehicle() && other.isPushable()`:
`other.push(dx, 0, dz)`. `Entity.push(x, y, z)` (5306–5328): if all finite, `motion +=
(x, y, z)` and `needsSync = true`.

**`AbstractMinecart.push(other)`** (lines 1040–1156), called when another entity pushes
into the cart (a player's `pushEntities`: **Not read**, `LivingEntity`) or a cart's
`pushAndPickupEntities` reaches it: return on the client, if either has `noPhysics`,
or if `hasPassenger(other)`. `dx = other.x - x`, `dz = other.z - z`; `d2 = dx*dx +
dz*dz`; if `d2 >= 9.999999747378752E-5` (`1.0E-4f`): `d = sqrt(d2)`; `dx /= d`; `dz /=
d`; `k = 1.0/d`, capped at 1.0; `dx *= k`; `dz *= k`; `dx *= 0.10000000149011612`;
`dz *= …` (`0.1f`); `dx *= 0.5`; `dz *= 0.5`; if `other` is an `AbstractMinecart`:
`pushOtherMinecart(other, dx, dz)`; else `this.push(-dx, 0, -dz)`; `other.push(dx /
4.0, 0, dz / 4.0)`.

**`pushOtherMinecart(other, dx, dz)`** (lines 1158–1345), old behaviour:
1. `rx = other.x - x`, `rz = other.z - z`; `a = (rx, 0, rz).normalize()`; `b =
   ((double) Mth.cos((double)(yRot * 0.017453292f)), 0, (double) Mth.sin(…)).normalize()`
   (the angle is multiplied in `float`, widened, and `Mth.cos:(D)F` / `Mth.sin:(D)F`
   return a `float`, widened again; lines 1193–1212; the game's `Mth.sin` is a table
   lookup, **Not read**). `dot = Math.abs(a.dot(b))`; if `dot < 0.800000011920929`
   (`0.8f`): return (old behaviour only). Carts push each other only nearly end to end
   along this cart's yaw.
2. `v = motion`, `w = other.motion`.
3. If `other.isFurnace() && !this.isFurnace()`: `motion = v.multiply(0.2, 1.0, 0.2)`;
   `this.push(w.x - dx, 0, w.z - dz)`; `other.motion = w.multiply(0.95, 1.0, 0.95)`.
4. Else if `!other.isFurnace() && this.isFurnace()`: `other.motion = w.multiply(0.2,
   1.0, 0.2)`; `other.push(v.x + dx, 0, v.z + dz)`; `motion = v.multiply(0.95, 1.0,
   0.95)`.
5. Else: `mx = (w.x + v.x) / 2.0`, `mz = (w.z + v.z) / 2.0`; `motion = v.multiply(0.2,
   1.0, 0.2)`; `this.push(mx - dx, 0, mz - dz)`; `other.motion = w.multiply(0.2, 1.0,
   0.2)`; `other.push(mx + dx, 0, mz + dz)`.

Order matters: a cart's `pushAndPickupEntities` changes other carts' motion in the
middle of the entity phase, so the outcome depends on the order carts tick (ADR-0028's
order by key; the game's is its entity list's). Named difference, judged by
distribution.

**Collisions in `move`**: see finding 4.

### 3.11 Getting off: `getDismountLocationForPassenger(passenger)` (lines 203–411)

1. `dir = getMotionDirection()` (3.4); if its axis is `Y` (never, for a horizontal
   yaw) → `VehicleEntity`'s (section 1).
2. `offsets = DismountHelper.offsetsForDirection(dir)` (section 1); `base =
   blockPosition()`.
3. For each `pose` of `passenger.getDismountPoses()` (**Not read**, `LivingEntity`;
   **Inferred** for a player: standing, crouching, swimming): `dims =
   passenger.getDimensions(pose)`; `half = Math.min(dims.width(), 1.0f) / 2.0f`
   (float); for each `dy` of `POSE_DISMOUNT_HEIGHTS[pose]` (`STANDING: [0, 1, -1]`,
   `CROUCHING: [0, 1, -1]`, `SWIMMING: [0, 1]`, static init lines 1542–1565): for each
   `[ox, oz]` of `offsets`: `m = (base.x + ox, base.y + dy, base.z + oz)`; `floor =
   level.getBlockFloorHeight(DismountHelper.nonClimbableShape(level, m), () ->
   DismountHelper.nonClimbableShape(level, m.below()))`; skip unless
   `DismountHelper.isBlockFloorValid(floor)`; `box = AABB(-half, 0, -half, half,
   dims.height(), half)` (each `f2d`); `at = Vec3.upFromBottomCenterOf(m, floor)`; if
   `DismountHelper.canDismountTo(level, passenger, box.move(at))`:
   `passenger.setPose(pose)`; return `at`.
4. Otherwise: `top = getBoundingBox().maxY`; `m = (base.x, top, base.z)` (the double
   setter floors); for each `pose`: `h = (double) dims.height()`; `c = Mth.ceil(top -
   m.y + h)`; `ceiling = DismountHelper.findCeilingFrom(m, c, p -> level.
   getBlockState(p).getCollisionShape(level, p))`; if `top + h <= ceiling`:
   `passenger.setPose(pose)`; stop. Then return `VehicleEntity`'s (section 1).

Offsets are tried beside the cart, square to its direction first (`offsetsForDirection`,
section 1).

### 3.12 Riding a minecart: `Minecart.interact(player, hand, at)` (Minecart.txt 15–40)

If `!player.isSecondaryUseActive()` (not sneaking) and `!isVehicle()`:
`playerRotationOffset = rotationOffset` (client cosmetics); on the server `return
player.startRiding(this) ? SUCCESS_SERVER : CONSUME`; on the client `CONSUME`. Else
`PASS`. One passenger only (the base `canAddPassenger`, section 1). `positionRider`
(Minecart.txt 132–177) adds only a client turn under the experiment.

### 3.13 Synched data and what is saved

Entity's 0–7 (section 8), then:

| id | owner | name | serializer | default |
|---|---|---|---|---|
| 8 | VehicleEntity | `DATA_ID_HURT` | `INT` | 0 |
| 9 | VehicleEntity | `DATA_ID_HURTDIR` | `INT` | 1 |
| 10 | VehicleEntity | `DATA_ID_DAMAGE` | `FLOAT` | 0.0f |
| 11 | AbstractMinecart | `DATA_ID_CUSTOM_DISPLAY_BLOCK` | `OPTIONAL_BLOCK_STATE` | empty |
| 12 | AbstractMinecart | `DATA_ID_DISPLAY_OFFSET` | `INT` | `getDefaultDisplayOffset()` = 6 |

(VehicleEntity.txt 153–173, 251–265; AbstractMinecart.txt 140–157, 1525–1541.) Each
subclass adds its own after 12 (the furnace cart's fuel flag; the command cart's
command and output: 3.17, 3.19). `getDisplayBlockState()` = the custom state or
`getDefaultDisplayBlockState()` (air for the plain cart; the subclasses' blocks).

Saved (`addAdditionalSaveData`, lines 1010–1039; read 981–1009): `DisplayState` (a
`BlockState` codec) only when custom; `DisplayOffset` only when not the default;
`FlippedRotation` (boolean, `flipped`); `HasTicked` (boolean) **holding the field
`firstTick`**, read back into `firstTick` with default false: the name is the
opposite of the value but it round-trips, and a cart read from a save without it
counts as having ticked. Only the new behaviour reads `firstTick`. `onRails` is not
saved (it is set every tick before use). VehicleEntity's hurt data are not saved
(**Inferred**: no save methods in VehicleEntity.txt).

The client: updateInterval 3, tracking range 8 chunks, `trackDeltas` true (the
repo's `ENTITY_TYPE_INFO`; width `0.98f`, height `0.7f`, eye `0.595f`, the same for all
seven carts). Under the old behaviour `ServerEntity` sends a cart as any entity
(n4-brief 9); the minecart branch of `sendChanges` (ServerEntity.txt 312–331) is for
the new behaviour only.

### 3.14 `VehicleEntity` (VehicleEntity.txt, 266 lines)

Abstract; extends `Entity`. Superclass of `AbstractMinecart` and `AbstractBoat`.

**Synched data** (lines 3–7, defined in `defineSynchedData` lines 153–173, ids from the
static init lines 251–265), continuing after `Entity`'s eight (indices 0–7):

| index | field | serializer | default |
|---|---|---|---|
| 8 | `DATA_ID_HURT` (hurt time) | `INT` | `0` |
| 9 | `DATA_ID_HURTDIR` | `INT` | `1` |
| 10 | `DATA_ID_DAMAGE` | `FLOAT` | `0.0f` |

None is saved (no `addAdditionalSaveData` here; `AbstractMinecart`'s saves none of
them, lines 1010–1038).

**`hurtServer(level, source, amount)`** (lines 22–96), in order:

```
if isRemoved(): return true
if isInvulnerableToBase(source): return false
setHurtDir(-getHurtDir())                    // 1 → -1 → 1 …
setHurtTime(10)
markHurt()                                   // Entity: syncVelocity = true (Entity.txt 5330–5335)
setDamage(getDamage() + amount * 10.0f)      // float: fmul then fadd
gameEvent(ENTITY_DAMAGE, source.getEntity())
creative = source.getEntity() instanceof Player p && p.getAbilities().instabuild
if (!creative && getDamage() > 40.0f) || shouldSourceDestroy(source):
    destroy(level, source)
else if creative:
    discard()
return true
```

- The creative test reads `getEntity()`, the **causing** entity (an arrow's owner, not the
  arrow): an arrow shot by a creative player breaks a cart as a punch does.
- `> 40.0f` is a float comparison (`fcmpl`, `ifgt`): exactly 40 does not break.
- The damage is added before either test, in creative too: the client is sent the new
  hurt time, direction and damage (`SetEntityData` of indices 8, 9, 10) before the
  removal. Clustine may send them or not; the removal follows in the same tick.
- `isInvulnerableToBase(source)` (Entity.txt 8588–8621): true if removed; or if
  `isInvulnerable()` (the `Invulnerable` tag) and the source is neither in
  `#minecraft:bypasses_invulnerability` nor a creative player
  (`DamageSource.isCreativePlayer`, not read); or a fire source (`#is_fire`) on a
  fire-immune type (no vehicle is); or a fall source (`#is_fall`) on a type in
  `#minecraft:fall_damage_immune`. So an `Invulnerable` cart is still broken by a creative
  player.
- **Who calls it**: a player's attack (the `Attack` packet, section 2.8 → 
  `Player.attack` → `hurtOrSimulate`; not read here), projectiles,
  explosions, lava (`AbstractMinecart.tick` → `lavaHurt`). The amount from a player's
  attack is the attack damage scaled by the attack strength (`Player.attack`, not read):
  a fist's `1.0` adds 10, so five quick fist hits break a cart in survival, minus the
  decay of one a tick.

`shouldSourceDestroy(source)` (lines 98–101): false. Overridden only by `MinecartTNT`.

`destroy(level, source)` (lines 235–242): `destroy(level, getDropItem())`.

`destroy(level, item)` (lines 122–151):

```
kill(level)                                  // Entity.txt 650–658: remove(KILLED); gameEvent(ENTITY_DIE)
if !level.gameRules.get(ENTITY_DROPS): return
stack = new ItemStack(item)
stack.set(CUSTOM_NAME, getCustomName())      // the vehicle's name goes onto the item (null: none)
spawnAtLocation(level, stack)
```

`spawnAtLocation(level, stack)` (Entity.txt 6374–6435): offset `(0, 0.0f widened, 0)`;
if the stack is empty, nothing; else `new ItemEntity(level, getX(), getY() + 0.0,
getZ(), stack)` (the constructor's draws, n4-brief section 8.4), `setDefaultPickUpDelay()`
(10), `addFreshEntity`. The item is born at the vehicle's exact position, so in its
chunk. The order is **kill first, then the item**: a container's contents (by `remove`)
come before the vehicle's item.

`ignoreExplosion(explosion)` (lines 103–120): true when the explosion's indirect source
is a `Mob` and `mobGriefing` is false (N6).

`hurtClient` returns true (lines 17–20): the client's own prediction; nothing for the
server. `getDimensionChangingDelay()` is 10 (portals; later).

**What the client sees of a hit** (**Client** for the drawing): `SetEntityData` with
the hurt time, direction and damage, and, from `markHurt`'s `syncVelocity`, a
`SetEntityMotion` to the trackers (n4-brief section 9, end of `sendChanges`). The client
rocks the cart or boat from hurt time, damage and direction. No hurt-animation packet is
sent for vehicles (**Inferred**: `ClientboundHurtAnimationPacket` comes from
`LivingEntity`; `AbstractMinecart.animateHurt`, lines 430–449, is the client's
handling of one and repeats the server's three changes, `damage + damage * 10`).

**Decay**: `AbstractMinecart.tick` (lines 486–536) begins `if hurtTime > 0: hurtTime -=
1; if damage > 0.0f: damage -= 1.0f` (float), so a cart left alone is whole again
after `damage` ticks. (Boats: section 4.2.)

---------------------------------------------------------------------------------------

### 3.15 What `AbstractMinecart` adds beside movement (AbstractMinecart.txt)

Movement, rails, pushing and riding are in 3.1 to 3.13; here what the kinds use.

- **Behaviour** (constructor lines 23–50): `useExperimentalMovement(level)` (lines
  1406–1412: the level's enabled features contain `MINECART_IMPROVEMENTS`) chooses
  `NewMinecartBehavior`, otherwise `OldMinecartBehavior`. The feature is the experimental
  data pack `minecart_improvements` (`generated/data/minecraft/datapacks/
  minecart_improvements`), so a plain world uses **`OldMinecartBehavior`**. The
  constructor also sets `flipped = false`, `blocksBuilding = true`.
- **Synched data** (`defineSynchedData` lines 140–157, static init lines 1527–1544),
  after `VehicleEntity`'s:

  | index | field | serializer | default |
  |---|---|---|---|
  | 11 | `DATA_ID_CUSTOM_DISPLAY_BLOCK` | `OPTIONAL_BLOCK_STATE` | `Optional.empty()` |
  | 12 | `DATA_ID_DISPLAY_OFFSET` | `INT` | `getDefaultDisplayOffset()` of the kind |

  The shown block is `getDisplayBlockState()` (lines 1347–1355): the custom one if set,
  else `getDefaultDisplayBlockState()` of the kind, which the client works out itself
  (**Client**); the server never sends a default.
- **Defaults per kind**:

  | kind | `getDefaultDisplayBlockState()` | `getDefaultDisplayOffset()` |
  |---|---|---|
  | `AbstractMinecart` (`minecart`) | `air` (lines 1366–1370) | 6 (lines 1382–1385) |
  | `MinecartChest` | `chest[facing=north]` (MinecartChest.txt 29–37) | 8 (39–42) |
  | `MinecartFurnace` | `furnace[facing=north, lit=hasFuel()]` (MinecartFurnace.txt 339–353) | 6 |
  | `MinecartTNT` | `tnt` (MinecartTNT.txt 36–40) | 6 |
  | `MinecartHopper` | `hopper` default state (MinecartHopper.txt 23–27) | 1 (29–32) |
  | `MinecartSpawner` | `spawner` (MinecartSpawner.txt 56–60) | 6 |
  | `MinecartCommandBlock` | `command_block` default (MinecartCommandBlock.txt 93–97) | 6 |

- **Saved** (`addAdditionalSaveData` lines 1010–1038, `readAdditionalSaveData` lines
  981–1008): `DisplayState` (a block state by `BlockState.CODEC`, only if a custom one is
  set); `DisplayOffset` (an int, only if not the kind's default; read with the default);
  `FlippedRotation` (boolean, default false); `HasTicked`, which holds `Entity.firstTick`
  (written as is, read with default false; the field is set false at the end of every
  tick, line 87–89). Nothing of damage.
- **Gravity** (`getDefaultGravity` lines 476–485): `0.005` in water, else `0.04`
  (doubles). No kind overrides it. **Air drag** off the rails, when not on ground:
  `getAirDrag()` = `0.95f`, widened (lines 759–762; `comeOffTrack` lines 707–757). The
  block speed factor on a rail is `1.0f` (lines 412–428).
- **Natural slowdown** (`applyNaturalSlowdown` lines 959–979, used by the behaviour on a
  rail): `v.multiply(f, 0.0, f)` with `f = behaviour.getSlowdownFactor()`
  (`OldMinecartBehavior.txt` 1514–1523: `0.997` with a passenger, else `0.96`), then
  `scale(0.949999988079071)` (`0.95f` widened) in water. The `y` is zeroed. The furnace
  and the container kinds override it (3.17, 3.18).
- **Maximum speed** (`OldMinecartBehavior.getMaxSpeed`, lines 1503–1512): `0.2` in water,
  `0.4` otherwise; the furnace scales it (3.17).
- `activateMinecart(level, x, y, z, powered)` (lines 616–618): nothing. The old
  behaviour calls it every tick the cart is on a rail whose block is `activator_rail`,
  after `moveAlongTrack`, with that rail's `powered` (`OldMinecartBehavior.txt` 104–157).
  It is called on unpowered activator rails too, with `false`.
- `isRideable()` false (lines 1416–1419), `isFurnace()` false (1421–1424): the old
  behaviour's passenger push reads the first, `pushOtherMinecart` the second (3.17).
- `isPickable()` = not removed (lines 451–459); `isPushable()` true (166–169);
  `canCollideWith` = `AbstractBoat.canVehicleCollide` (159–164; section 4.9).
- `getPickResult()` (the creative middle-click) is each kind's own item (below).

---------------------------------------------------------------------------------------

### 3.16 `Minecart`, the one that is ridden (Minecart.txt, 178 lines)

- `interact(player, hand, pos)` (lines 15–40):

  ```
  if player.isSecondaryUseActive() || isVehicle(): return PASS   // sneaking, or someone already in
  playerRotationOffset = rotationOffset                            // client rotation bookkeeping
  if !level.isClientSide():
      return player.startRiding(this) ? SUCCESS_SERVER : CONSUME
  return CONSUME
  ```

  `isVehicle()` is "has a passenger" (`Entity`): one seat. No item in hand is looked at:
  a player holding anything rides. `startRiding`'s checks are section 1.2's.
- `activateMinecart(level, x, y, z, powered)` (lines 55–80):

  ```
  if powered:
      if isVehicle(): ejectPassengers()
      if getHurtTime() == 0:
          setHurtDir(-getHurtDir()); setHurtTime(10); setDamage(50.0f); markHurt()
  ```

  On a powered activator rail: riders are put off every tick (a player cannot stay on);
  every ten ticks (hurt time counts 10 down to 0 in the tick, 3.14) the cart rocks, its
  damage is **set** to 50 and a `SetEntityMotion` goes out. Nothing breaks it here, but
  damage stays between 41 and 50 while it sits there, so one survival hit (+10 or more,
  then `> 40`) breaks it.
- `getDropItem()` = `minecart` (lines 42–45); pick result the same (47–53).
- `isRideable()` true (82–85).
- `tick()` (87–130) and `positionRider` (132–177) add only client-side rotation
  bookkeeping (`rotationOffset`, `playerRotationOffset`; the second only with the
  experimental behaviour): **Client**, nothing on the server.
- No synched data of its own.

---------------------------------------------------------------------------------------

### 3.17 `MinecartFurnace` (MinecartFurnace.txt, 364 lines)

Constants (lines 3–15): `FUEL_TICKS_PER_ITEM = 3600`, `MAX_FUEL_TICKS = 32000`,
`DEFAULT_FUEL = 0`, `DEFAULT_PUSH = Vec3.ZERO`. Fields: `int fuel` (0), `Vec3 push`
(zero) (constructor 17–29).

**Synched data** (lines 36–47, static 355–363): index **13**, `DATA_ID_FUEL`, `BOOLEAN`,
default `false` ("has fuel"; it lights the furnace the client draws).

**`tick()`** (lines 49–103):

```
super.tick()                                 // AbstractMinecart.tick: decay, the behaviour's move …
if !clientSide:
    if fuel > 0: fuel -= 1
    if fuel <= 0: push = Vec3.ZERO
    setHasFuel(fuel > 0)
if hasFuel() && random.nextInt(4) == 0:       // the ENTITY's random, on the server too
    level.addParticle(LARGE_SMOKE, x, y + 0.8, z, 0, 0, 0)   // nothing on a ServerLevel (**Inferred**)
```

The draw is made on the server every tick the flag is set (read after `setHasFuel`, so
in every tick that ends with fuel). It changes nothing on the server but the random's
state, which Clustine's per-tick streams make moot (ADR-0028 section 4): name it, do not
draw it, or draw it; it is unobservable.

**Fuelling**: `interact(player, hand, pos)` (lines 208–225):

```
stack = player.getItemInHand(hand)
if addFuel(player.position(), stack): stack.consume(1, player)   // creative: not shrunk (**Inferred**, ItemStack.consume not read)
return SUCCESS                                                   // always, with any item or none
```

`addFuel(playerPos, stack)` (lines 227–259):

```
if !stack.is(#minecraft:furnace_minecart_fuel) || fuel + 3600 > 32000: return false
fuel += 3600
if fuel > 0: push = position().subtract(playerPos).horizontal()   // (dx, 0, dz), NOT normalised
return true
```

The tag is `generated/data/minecraft/tags/item/furnace_minecart_fuel.json`: `coal`,
`charcoal`. So at most 8 items (8 × 3600 = 28800; a ninth would make 32400 > 32000),
each 3600 ticks (three minutes). The push points from the player to the cart, and its
length is their horizontal distance (a player 2.5 blocks away gives a push of length
2.5). A furnace minecart is never ridden (`isRideable` false, and the interaction answers
`SUCCESS` before anything else could).

**Its slowdown** replaces the plain one while it pushes (`applyNaturalSlowdown`, lines
136–175), called by the behaviour each tick on a rail with the motion `v`:

```
if push.lengthSqr() > 1.0E-7:
    push = calculateNewPushAlong(v)
    w = v.multiply(0.8, 0.0, 0.8).add(push)
    if isInWater(): w = w.scale(0.1)
else:
    w = v.multiply(0.98, 0.0, 0.98)
return super.applyNaturalSlowdown(w)          // AbstractMinecart's: × (f, 0, f), f 0.997/0.96; × 0.95f in water
```

`calculateNewPushAlong(v)` (lines 177–206):

```
if push.horizontalDistanceSqr() > 1.0E-4 && v.horizontalDistanceSqr() > 0.001:
    return push.projectedOn(v).normalize().scale(push.length())
return push
```

So the push keeps its length and turns to lie along the cart's motion (with the sign of
their dot product: `projectedOn` is `v · (push·v / v·v)`, **Inferred**, `Vec3` not read),
which is how it follows curves. A cart standing still keeps the push as it was given
(from the player), and the motion becomes `push` (`0.8 · 0 + push`), which the
behaviour then limits to the maximum speed. `isFurnace()` is true (lines 31–34): no
passenger push, and in `pushOtherMinecart` (AbstractMinecart.txt 1158–1346, 3.10) a furnace cart meeting another cart keeps going: the one that is not a
furnace is slowed to `0.2` of its motion and pushed by the furnace's motion minus the
difference, the furnace keeps `0.95` (lines 1234–1290).

**Maximum speed** (lines 105–121): `super.getMaxSpeed(level) * 0.75` in water, else
`* 0.5`: with the old behaviour `0.15` in water, `0.2` elsewhere (half a plain cart's).

**Saved** (lines 261–317): `PushX`, `PushZ` (doubles; read with default 0, the `y` is 0),
`Fuel` (a short, `(short) fuel`; read with `getShortOr(…, 0)`). The `has fuel` flag is
not saved; the next tick sets it again (so a loaded cart with fuel starts its first
tick with the flag false, and is drawn unlit for a tick).

`getDropItem()` = `furnace_minecart` (lines 123–126); pick result the same.

---------------------------------------------------------------------------------------

### 3.18 Containers: `ContainerEntity`, `AbstractMinecartContainer`, `MinecartChest`, and the chest boat

`ContainerEntity` (ContainerEntity.txt, 315 lines) is an interface of default methods
for a vehicle that is a `Container` and a `MenuProvider`. Its implementers keep
`itemStacks` (a `NonNullList<ItemStack>`), `lootTable` (a `ResourceKey<LootTable>` or
null) and `lootTableSeed` (a long).

**Saved** (`addChestVehicleSaveData`, lines 29–56; `readChestVehicleSaveData`, lines
58–85):

```
write: if lootTable != null:
           putString("LootTable", lootTable.identifier().toString())
           if lootTableSeed != 0: putLong("LootTableSeed", lootTableSeed)
       else:
           ContainerHelper.saveAllItems(out, itemStacks)        // "Items": slots, by index (not read)
read:  clearItemStacks()                                       // a new list of getContainerSize() empties
       lootTable = read("LootTable", LootTable.KEY_CODEC) or null
       lootTableSeed = getLongOr("LootTableSeed", 0)
       if lootTable == null: ContainerHelper.loadAllItems(in, itemStacks)
```

So a container with a loot table saves no items. A creative player's chest minecart or
chest boat has none (made from the item; a loot table comes from structures or
commands), so N5 meets only `Items`.

**Loot tables** (`unpackChestVehicleLootTable(player)`, lines 125–182): if a table is set
and the server is there, it is looked up, the `GENERATE_LOOT` trigger fires for a
`ServerPlayer`, the table is cleared, and `LootTable.fill(container, params, seed)` fills
it with `ORIGIN` = position (and luck and `THIS_ENTITY` for a player), set `CHEST`. Every
read or write of a slot (`getChestVehicleItem`, `setChestVehicleItem`,
`removeChestVehicleItem…`, `clearChestVehicleContent`) unpacks first with a null player.
Not needed in N5 (no loot-table carts are placed); name it.

**Destroyed** (`chestVehicleDestroyed(source, level, entity)`, lines 87–114):

```
if !gameRules.get(ENTITY_DROPS): return
Containers.dropContents(level, entity, this)
if source.getDirectEntity() instanceof Player p: PiglinAi.angerNearbyPiglins(level, p, true)
```

**Opened** (`interactWithContainerVehicle(player)`, lines 116–123): `player.openMenu(this)`;
`SUCCESS`. Opening and the menu (`ChestMenu.threeRows`, `HopperMenu`), `stillValid`
(within interaction range of the box inflated by 4.0, lines 300–314), the `CONTAINER_OPEN`
/ `CONTAINER_CLOSE` game events and the piglins are **N6 or later**: in N5 the
interaction can answer as the game does and open nothing (a named difference), or open
the menu if the step takes containers.

**`AbstractMinecartContainer`** (AbstractMinecartContainer.txt, 243 lines):

- `itemStacks` starts as `NonNullList.withSize(36, EMPTY)` (constructor lines 9–20),
  whatever the kind's size; `clearItemStacks` (lines 234–242, called by loading) makes it
  `getContainerSize()` long.
- `destroy(level, source)` (lines 22–33): `super.destroy(level, source)` (3.14: kill, which
  drops the contents through `remove`; then the cart's item if `ENTITY_DROPS`), then
  `chestVehicleDestroyed(source, level, this)`.
- `remove(reason)` (lines 83–100): on the server, if `reason.shouldDestroy()` (`KILLED`,
  `DISCARDED`): `Containers.dropContents(level, this, this)`; then `super.remove`. Not for
  unloading or a change of dimension. **This is where the contents drop, in creative
  too** (3.0, item 9).
- `interact` (lines 122–127): `interactWithContainerVehicle(player)`: a chest or hopper
  cart is opened, never ridden.
- `applyNaturalSlowdown(v)` (lines 129–162), replacing `AbstractMinecart`'s (the
  behaviour's factor is not used):

  ```
  f = 0.98f
  if lootTable == null:
      i = 15 - AbstractContainerMenu.getRedstoneSignalFromContainer(this)   // 0..15
      f = f + (float) i * 0.001f                                            // float arithmetic
  if isInWater(): f = f * 0.95f
  return v.multiply((double) f, 0.0, (double) f)
  ```

  An empty chest cart rolls with `0.98f + 0.015f` (as floats, then widened), a full one
  with `0.98f`. `getRedstoneSignalFromContainer` is the comparator's reading of a
  container (not read; **Inferred**: the sum over slots of `count / maxStackSize`, over
  the size, mapped to 0–15 with 1 for anything not empty).
- `createMenu` (lines 180–198): null for a spectator when a loot table is set; else
  unpacks the loot table and makes the kind's menu.

**`MinecartChest`** (MinecartChest.txt, 93 lines): size 27 (lines 24–27); drop and pick
`chest_minecart` (11–22); display block and offset (3.15); `interact` (65–92): open as
above, and if that consumed the action, on the server the `CONTAINER_OPEN` game event and
`PiglinAi.angerNearbyPiglins(level, player, true)`. No synched data of its own.

**The chest boat side** (`AbstractChestBoat`, AbstractChestBoat.txt, 302 lines; the
classes `ChestBoat` and `ChestRaft` add only their constructors):

- 27 slots (constructor lines 11–23 makes the list 27 long; `getContainerSize` lines
  168–171); one passenger (`getMaxPassengers` 1, lines 30–33) seated with
  `getSinglePassengerXOffset` `0.15f` (lines 25–28).
- Saved: `AbstractBoat`'s, then `addChestVehicleSaveData` (lines 35–53).
- `destroy(level, source)` (lines 55–67): `destroy(level, getDropItem())` (3.14; the chest
  boat's item, from the type's supplier), then `chestVehicleDestroyed`.
- `remove(reason)` (lines 69–86): drops the contents when `shouldDestroy()`, as the carts.
- `interact` (lines 88–135):

  ```
  r = AbstractBoat.interact(player, hand, pos)        // section 4.6: riding, leads …
  if r != PASS: return r
  if canAddPassenger(player) && !player.isSecondaryUseActive(): return PASS
  r = interactWithContainerVehicle(player)            // open
  if r.consumesAction() && server: gameEvent(CONTAINER_OPEN, player); angerNearbyPiglins(…)
  return r
  ```

  So a chest boat opens when the player sneaks, or when its seat is taken; otherwise
  the boat's own interaction seats the player.
- No synched data beyond `AbstractBoat`'s.

---------------------------------------------------------------------------------------

### 3.19 The kinds left for N6 or later

Named, with what they would do in N5 if placed (3.0, item 13).

**`MinecartTNT`** (MinecartTNT.txt, 526 lines). Fields: `fuse` (`-1`, not primed),
`explosionPowerBase` `4.0f`, `explosionSpeedFactor` `1.0f`, `ignitionSource` (constructor
lines 19–35). No synched data: priming reaches the client as entity event 70
(`broadcastEntityEvent`, `handleEntityEvent` lines 282–295).

- `tick()` (lines 42–97): `super.tick()`; if `fuse > 0`: `fuse -= 1` (and a smoke
  particle, nothing on the server); else if `fuse == 0`: `explode(ignitionSource,
  horizontalDistanceSqr(motion))`; then if `horizontalCollision` and
  `horizontalDistanceSqr(motion) >= 0.009999999776482582` (`0.01f` widened): explode.
- `activateMinecart(…, powered)` (lines 270–281): if powered and `fuse < 0`: `primeFuse(null)`.
- `primeFuse(source)` (lines 296–358): nothing if the `tntExplodes` rule is off; `fuse =
  80`; on the server: keep an explosion source of the igniter; entity event 70; unless
  silent, `TNT_PRIMED` (blocks, volume 1, pitch 1).
- `hurtServer` (lines 99–131): a burning arrow as the direct entity explodes it at once
  (`explode(explosion source, arrow motion lengthSqr)`); then `AbstractMinecart.hurtServer`.
- `shouldSourceDestroy` (lines 494–498) = `damageSourceIgnitesTnt` (lines 500–526): a
  burning projectile as the direct entity, or a source in `#is_fire` or `#is_explosion`.
- `destroy(level, source)` (lines 133–170): if the source ignites TNT or
  `horizontalDistanceSqr(motion) >= 0.01f` widened: if `fuse < 0`: `primeFuse(source)`;
  `fuse = random.nextInt(20) + random.nextInt(20)` (entity's random; overrides the 80);
  return (no item). Otherwise `destroy(level, getDropItem())` (`tnt_minecart`).
- `causeFallDamage(fallDistance, …)` (lines 246–269): if `fallDistance >= 3.0`:
  `explode(ignitionSource, (fallDistance / 10.0)²)`.
- `explode(source, speedSqr)` (lines 184–245): if `tntExplodes`: `level.explode(this,
  source, null, x, y, z, (float)(explosionPowerBase + explosionSpeedFactor *
  random.nextDouble() * 1.5 * min(sqrt(speedSqr), 5.0)), false, TNT)`, then `discard()`;
  else `discard()` only if primed.
- Saved (lines 431–493): `fuse` (int), `explosion_power` and `explosion_speed_factor`
  (floats, written only when not the defaults, read clamped to `[0, 128]`).
- A creative player's punch discards it, never primes it (`shouldSourceDestroy` false for
  a punch).

**`MinecartHopper`** (MinecartHopper.txt, 231 lines): a `Hopper` of 5 slots (lines
34–37), `enabled` (default true, saved as `Enabled`). `activateMinecart` (lines 39–55):
`enabled = !powered` (set only when it changes): a powered activator rail turns it off.
`tick` (lines 94–103) and each step along the track (`makeStepAlongTrack`, 105–116) call
`tryConsumeItems` (118–142): on the server, alive, enabled and not yet this tick:
`HopperBlockEntity.suckInItems(level, this)`, and on success mark it consumed. Its
`suckInItems` (143–183) takes from the container above and from item entities in its box
inflated by `0.25, 0, 0.25` (`HopperBlockEntity.addItem`). Drop `hopper_minecart`. No
synched data. Its pick-up of items reads entities across a line (N6, with hoppers).

**`MinecartSpawner`** (MinecartSpawner.txt, 133 lines): a `BaseSpawner` ticked each tick
on the server at `blockPosition()` (lines 99–107, 124–133); saved by `BaseSpawner.save`;
entity events go to the spawner. Drops a plain `minecart`. No item places it. N7.

**`MinecartCommandBlock`** (MinecartCommandBlock.txt, 192 lines): synched data **13**
`DATA_ID_COMMAND_NAME` (`STRING`, `""`) and **14** `DATA_ID_LAST_OUTPUT` (`COMPONENT`,
`CommonComponents.EMPTY`) (lines 40–56, static 181–192). `activateMinecart` (105–126): if
powered and `tickCount - lastActivated >= 4`: run the command, `lastActivated =
tickCount` (not saved). `interact` (127–143) opens the command screen for players who may
use game-master blocks. Drops a plain `minecart`, picks `command_block_minecart`. Later.

---------------------------------------------------------------------------------------

## 4. Boats and rafts

Classes: `AbstractBoat` (AbstractBoat.txt), `Boat`, `Raft`, `AbstractChestBoat`,
`ChestBoat`, `ChestRaft`, `AbstractBoat$Status`, `BoatItem` (BoatItem.txt), with what
they inherit from `VehicleEntity` (VehicleEntity.txt) and `Entity` (Entity.txt). Line
ranges are of those files.

### 4.0 Findings of this section

1. **A boat ridden by a player is still driven by that player's client in 26.3.**
   `AbstractBoat.getControllingPassenger()` (lines 2189–2205) is the first passenger if
   it is a `Player`, else `Entity`'s, which is `null` (Entity.txt 10152–10155): only a
   player ever controls a boat, and only from the first seat. `Entity.isClientAuthoritative()`
   (Entity.txt 10354–10368) is `controller != null && controller.isClientAuthoritative()`,
   and `isLocalInstanceAuthoritative()` (10322–10337) is, on the server,
   `!isClientAuthoritative()`. (**Inferred**: `Player.isClientAuthoritative()` is true;
   `LivingEntity`/`Player` not read.) So with a player in front the server's `tick`
   sets the motion to zero and `deltaRotation` to 0 and does not float, steer or move
   the boat (4.2 step 6). The client runs `floatBoat`, `controlBoat` and `move`, and
   sends, every client tick, `ServerboundPaddleBoatPacket(left, right)` (lines
   453–468, `level.sendPacketToServer`) and `ServerboundMoveVehiclePacket` (section 2,
   `handleMoveVehicle`, ServerGamePacketListenerImpl.txt 669–). The server's
   `handlePaddleBoat` (ServerGamePacketListenerImpl.txt 4469–4492) only sets the two
   paddle data flags on `player.getControlledVehicle()` if it is an `AbstractBoat`.
   `AbstractBoat.setInput` (lines 2206–2221) is called by nothing on the server: the
   keys a player presses never reach the server's boat.
2. **A boat nobody (no player) controls is simulated by the server**: floating,
   gravity, drag, being pushed by water currents (`Entity.baseTick`'s fluid
   interaction), bubble columns, its own `move`, and picking up mobs. A boat a player
   has just left starts from zero motion (the server zeroed it every tick the player
   drove). N5 needs this simulation (4.4, 4.5), not only the client's moves.
3. **Boats no longer break when they fall, and do not hurt their riders.**
   `AbstractBoat.checkFallDamage` (lines 2077–2115) does not call `Entity`'s: it saves
   `lastYd = deltaMovement.y`; returns if the boat is a passenger; on the ground resets
   `fallDistance`; otherwise, if the fluid below `blockPosition()` is not water and
   `movedY < 0`, `fallDistance -= (double)(float)movedY`. Nothing reads that
   `fallDistance` to damage anything. (The planks-and-sticks drop of old versions is
   gone.) **Inferred**: a player riding a boat that falls takes no fall damage, as a
   passenger does not `move` itself.
4. **A boat is a solid collider for every other entity** (`canBeCollidedWith` true,
   lines 150–154), and collides with any entity that is a collider or pushable
   (`canVehicleCollide`, lines 132–149). `Entity.collide` gathers
   `level.getEntityCollisions(this, box.expandTowards(vec).expandTowards(0,
   maxUpStep, 0))` (Entity.txt 2862–2877) and `collectAllColliders` (3145–3155). The
   repo's `physics/collide.rs` has block shapes only (the N4 brief, section 10:
   "entity colliders (no boats or shulkers yet)"). N5 needs entity colliders: items and
   falling blocks land on boats, a server-simulated boat stops against a player's box
   (players are pushable), and a player stands on a boat (the client predicts that; the
   server's check of the player's move must agree, or the player is corrected).
5. **Boats are `Leashable` in 26.3, with a four-point leash** (`supportQuadLeash()`
   true, lines 898–902; `getQuadLeashOffsets`, 903–912). Leashes (section 7) therefore
   meet boats in N5 without any mob. A boat destroyed (removal that `shouldDestroy`)
   drops its leash (`remove`, lines 2058–2076).
6. **A boat blocks building**: the constructor sets `blocksBuilding = true` (lines
   59–76). **Inferred** (the level's obstruction check not read): a block cannot be
   placed into a boat's box. The repo's placement must count boats as obstructions.
7. **Paddle sounds are the server's.** The server's `tick` plays
   `entity.boat.paddle_water`/`paddle_land` with `level.playSound(null, …)` while a
   player paddles (4.2 step 9), from the paddle flags the client sent. (**Client**,
   **Inferred**: the client's own call passes a null player and so plays nothing
   locally.) Each sound draws the boat's random.
8. **`applyEffectsFromBlocks()` runs twice in a row in every boat tick** (lines
   473–476). The first consumes the tick's movements; the second, with none recorded,
   runs over `Movement(oldPosition(), position())` (the N4 brief, section 1.9). So
   every block a boat is inside acts on it twice a tick (a bubble column's
   `onAboveBubbleColumn` and its draw among them).
9. **Eleven woods**, `pale_oak` and `poplar` among them: 20 boat and chest boat types
   plus `bamboo_raft` and `bamboo_chest_raft`, 22 entity types (8.1, 8.4). Rafts differ
   from boats only in their seat height (4.8).
10. **Mannequins can be scooped into a drifting boat; armour stands cannot.** The
    pick-up of 4.6 takes any `LivingEntity` narrower than the boat that is not in
    `#minecraft:cannot_be_pushed_onto_boats` (which holds `player`). `ArmorStand.isPushable()`
    is false (ArmorStand.txt 326–329), so the pushable-entity selector leaves it out;
    `Mannequin` extends `Avatar` and does not override it (**Inferred**:
    `LivingEntity.isPushable` and `EntitySelector.pushableBy` not read), so a mannequin
    (0.6 wide) touched by a boat with no player in front is made to ride it.

### 4.1 Placing: `BoatItem.use(level, player, hand)` (BoatItem.txt lines 15–134)

1. `stack = player.getItemInHand(hand)`.
2. `hit = Item.getPlayerPOVHitResult(level, player, ClipContext.Fluid.ANY)` (not read:
   the reach is the player's block interaction range; `Fluid.ANY` so the ray stops at
   any fluid's surface, flowing water included). `MISS` → `PASS`.
3. **Entities in the way**: `view = player.getViewVector(1.0f)`; `list =
   level.getEntities(player, player.getBoundingBox().expandTowards(view.scale(5.0))
   .inflate(1.0), EntitySelector.CAN_BE_PICKED)`. If not empty: `eye =
   player.getEyePosition()`; for each `e`: if `e.getBoundingBox().inflate((double)
   e.getPickRadius()).contains(eye)` → `PASS`. (Only an entity whose box holds the
   player's eyes refuses the use; the 5.0 is fixed, not the reach.)
4. If `hit` is a `BLOCK` hit (the only other kind `getPlayerPOVHitResult` gives):
   - `boat = getBoat(level, hit, stack, player)` (lines 135–171):
     `entityType.create(level, EntitySpawnReason.SPAWN_ITEM_USE)`; if not null:
     `setInitialPos(hit.location.x, y, z)` (lines 77–94: `setPos` and `xo/yo/zo`
     the same, so the first tick's movement is from that point); on a server level
     `EntityType.createDefaultStackConfig(level, stack, player).apply(boat)` (not read:
     the stack's custom name and entity data components). `null` → `FAIL`.
   - `boat.setYRot(player.getYRot())`: the player's yaw exactly, not snapped.
   - if `!level.noCollision(boat, boat.getBoundingBox())` → `FAIL` (blocks, entity
     colliders and the world border, **Inferred**: `CollisionGetter.noCollision` not
     read). The boat is centred on the hit point: on a block's top face or a fluid's
     surface it stands on it; on a side face its box overlaps the block, so it fails.
   - on the server: `level.addFreshEntity(boat)`; `level.gameEvent(player,
     GameEvent.ENTITY_PLACE, hit.location)`; `stack.consume(1, player)` (**Inferred**,
     `ItemStack.consume` not read: a player with infinite materials keeps it).
   - `player.awardStat(Stats.ITEM_USED.get(this))`; `SUCCESS`.
5. Otherwise `PASS`.

Water and land are not told apart: the ray decides. On water the boat is placed at the
surface point the ray met; `floatBoat` settles it. A boat placed in the air above
nothing is not possible (the ray must hit something).

### 4.2 `AbstractBoat.tick()` (lines 369–590), in order

Server and client run the same method; what differs is `isLocalInstanceAuthoritative()`
(4.0, finding 1). "Authoritative" below means: on the server, no player controls the
boat; on the client, the local player does.

1. `oldStatus = status; status = getStatus()` (4.3).
2. If `status` is `UNDER_WATER` or `UNDER_FLOWING_WATER`: `outOfControlTicks += 1.0f`;
   else `outOfControlTicks = 0.0f`.
3. On the server: if `outOfControlTicks >= 60.0f` → `ejectPassengers()` (Entity.txt
   7009–7027: `stopRiding()` of each passenger, last first). This runs whoever
   controls the boat.
4. If `getHurtTime() > 0`: `setHurtTime(getHurtTime() - 1)`. If `getDamage() > 0`:
   `setDamage(getDamage() - 1.0f)`. (Synched data, so it is sent while it changes.)
5. `super.tick()`: `VehicleEntity` has no `tick`, so `Entity.tick()` = `baseTick()`
   (fire, fluid interaction and currents, portals; the N4 brief, section 3.2). Its
   fluid push changes the motion even when a player drives; step 6 then zeroes it.
6. **If authoritative**:
   - if `getFirstPassenger()` is not a `Player`: `setPaddleState(false, false)`;
   - `floatBoat()` (4.4);
   - on the client only: `controlBoat()` (4.5) and the paddle packet;
   - `move(MoverType.SELF, getDeltaMovement())` (the N4 brief, section 1.2; the
     boat's `maxUpStep` is 0; entity colliders count, finding 4).
   **Else** (on the server: a player drives): `setDeltaMovement(Vec3.ZERO)`;
   `deltaRotation = 0.0f`.
7. `applyEffectsFromBlocks()` twice (lines 473–476; finding 8).
8. `tickBubbleColumn()` (4.7).
9. **Paddles**, for `i` in 0, 1 (lines 481–560):
   - if `getPaddleState(i)` (lines 2116–2136: the data flag `DATA_ID_PADDLE_LEFT`
     (i = 0) or `_RIGHT`, **and** `getControllingPassenger() != null`):
     - if `!isSilent()` and `(double)(paddlePositions[i] % 6.2831855f) <=
       0.7853981852531433` and `(double)((paddlePositions[i] + 0.3926991f) %
       6.2831855f) >= 0.7853981852531433` (float `%` is Java `frem`, Rust `%` on
       `f32`): `sound = getPaddleSound()` (lines 810–828: status `IN_WATER`,
       `UNDER_WATER`, `UNDER_FLOWING_WATER` → `entity.boat.paddle_water`; `ON_LAND` →
       `entity.boat.paddle_land`; `IN_AIR` → none); if a sound: `view =
       getViewVector(1.0f)`; `dx = i == 1 ? -view.z : view.z`; `dz = i == 1 ? view.x
       : -view.x`; `level.playSound(null, getX() + dx, getY(), getZ() + dz, sound,
       getSoundSource(), 1.0f, 0.8f + 0.4f * random.nextFloat())`;
     - `paddlePositions[i] += 0.3926991f` (float).
   - else `paddlePositions[i] = 0.0f`.
   `paddlePositions` is not saved or synched, and starts at 0. With float arithmetic
   the condition holds, counting the ticks of a paddle's stroke from 0, at ticks 1 and
   2 (`π/8` and `π/4` are both in the window, the latter by equality), then 17, 33, …
   (computed with `f32` and `frem`; port the arithmetic, not the pattern).
10. **Pushing and picking up** (lines 412–590): `list = level.getEntities(this,
    getBoundingBox().inflate(0.20000000298023224, -0.009999999776482582,
    0.20000000298023224), EntitySelector.pushableBy(this))` (the constants are
    `0.2f` and `-0.01f` widened). If not empty: `pickUp = !level.isClientSide() &&
    !(getControllingPassenger() instanceof Player)`; for each `e`:
    - skip it if `e.hasPassenger(this)` (it is this boat's vehicle);
    - if `pickUp && getPassengers().size() < getMaxPassengers() && !e.isPassenger()
      && hasEnoughSpaceFor(e)` (lines 1743–1755: `e.getBbWidth() < getBbWidth()`)
      ` && e instanceof LivingEntity && !e.is(#minecraft:cannot_be_pushed_onto_boats)`
      → `e.startRiding(this)` (section 1);
    - else `push(e)` (4.9).
    Picking up mobs needs mobs (N7); in N5 only mannequins (finding 10).

### 4.3 Status: `getStatus`, `isUnderwater`, `checkInWater`, `getGroundFriction`

`AbstractBoat$Status` ordinals: `IN_WATER` 0, `UNDER_WATER` 1, `UNDER_FLOWING_WATER` 2,
`ON_LAND` 3, `IN_AIR` 4. `isUnderWater()` (lines 2222–2236) is `status ==
UNDER_WATER || status == UNDER_FLOWING_WATER` (the field, as of the last `getStatus`).

**`getStatus()`** (lines 913–946):
1. `s = isUnderwater()`; if not null: `waterLevel = getBoundingBox().maxY`; return `s`.
2. if `checkInWater()` → `IN_WATER`.
3. `f = getGroundFriction()`; if `f > 0.0f`: `landFriction = f`; return `ON_LAND`.
4. `IN_AIR`.

**`isUnderwater()`** (lines 1328–1430): `box = getBoundingBox()`; `top = box.maxY +
0.001`; x over `[floor(minX), ceil(maxX))`, y over `[floor(maxY), ceil(top))`, z over
`[floor(minZ), ceil(maxZ))` (loop order x, y, z; `Mth.floor`, `Mth.ceil`); for each
position whose fluid state is in `#minecraft:water`: if `top < (double)((float)y +
fluid.getHeight(level, pos))` (float add, then widen): if `fluid.isSource()` → mark
`source`; else return `UNDER_FLOWING_WATER` at once. At the end: `source ?
UNDER_WATER : null`.

**`checkInWater()`** (lines 1220–1327): x over `[floor(minX), ceil(maxX))`, y over
`[floor(minY), ceil(minY + 0.001))`, z over `[floor(minZ), ceil(maxZ))`; `waterLevel =
-Double.MAX_VALUE`; `in = false`; for each water position: `f = (float)y +
fluid.getHeight(level, pos)` (float); `waterLevel = Math.max((double)f, waterLevel)`;
`in |= box.minY < (double)f`. Return `in`. (`waterLevel` keeps the last value even when
the result is false.)

**`getGroundFriction()`** (lines 1050–1219): `b = AABB(minX, minY - 0.001, minZ, maxX,
minY, maxZ)` (a slab 0.001 thick under the boat); `x0 = floor(b.minX) - 1`, `x1 =
ceil(b.maxX) + 1`, `y0 = floor(b.minY) - 1`, `y1 = ceil(b.maxY) + 1`, `z0 =
floor(b.minZ) - 1`, `z1 = ceil(b.maxZ) + 1`; `shape = Shapes.create(b)`; `sum = 0.0f`,
`n = 0`; for x in `[x0, x1)`, for z in `[z0, z1)`:
- `edges = (x == x0 || x == x1 - 1 ? 1 : 0) + (z == z0 || z == z1 - 1 ? 1 : 0)`; skip
  the column if `edges == 2` (the four corners);
- for y in `[y0, y1)`: if `edges <= 0 || (y != y0 && y != y1 - 1)`: `state =
  getBlockState(x, y, z)`; skip a `LilyPadBlock`; if `Shapes.joinIsNotEmpty(
  state.getCollisionShape(level, pos).move(pos), shape, BooleanOp.AND)`: `sum +=
  block.getFriction()`, `n++`.
Return `sum / (float)n`: **`0.0f / 0.0f` is NaN when nothing touches**, and `NaN >
0.0f` is false, so the status is `IN_AIR` (keep the NaN, do not guard it). The outer
ring reaches one block beyond the box (for shapes taller or wider than their cell); it
reads up to `ceil(maxX) + 1`, i.e. as far as two blocks from the boat's centre.
`getFriction` per block is in the repo (`block_physics`, N4); ice 0.98, packed ice
0.98, blue ice 0.989, slime 0.8, others 0.6 (from the data, not this class).

**`getWaterLevelAbove()`** (lines 947–1049), used only by `floatBoat`'s landing: x over
`[floor(minX), ceil(maxX))`, z likewise, y from `floor(maxY)` up to (excluding)
`ceil(maxY - lastYd)`; for each y: `f = 0.0f`; for each x, z: if water, `f =
Math.max(f, fluid.getHeight(level, pos))` (float); **if `f >= 1.0f` go on to the next
y at once** (leaving the x and z loops); after the layer, if `f < 1.0f` return
`(float)pos.getY() + f` (the mutable position's y, which is this y). If every layer is
full, return `(float)(ceil(maxY - lastYd) + 1)`.

### 4.4 `floatBoat()` (lines 1436–1617; authoritative side only)

```
gravityY = -getGravity()           // getDefaultGravity() = 0.04 (lines 1431-1435); 0 if NoGravity
buoyancy = 0.0                     // double
drag = 0.05f                       // float
if oldStatus == IN_AIR && status != IN_AIR && status != ON_LAND:      // landing in water
    waterLevel = getY(1.0)                                            // top of the box
    y = (double)(getWaterLevelAbove() - getBbHeight()) + 0.101        // float subtract, widen, add
    if level.noCollision(this, getBoundingBox().move(0, y - getY(), 0)):
        setPos(getX(), y, getZ())
        setDeltaMovement(getDeltaMovement().multiply(1.0, 0.0, 1.0))
        lastYd = 0.0
    status = IN_WATER
    return                                                            // no drag, no gravity this tick
if status == IN_WATER:
    buoyancy = (waterLevel - getY()) / (double)getBbHeight()
    drag = 0.9f
else if status == UNDER_FLOWING_WATER:
    gravityY = -7.0E-4
    drag = 0.9f
else if status == UNDER_WATER:
    buoyancy = 0.009999999776482582                                   // 0.01f widened
    drag = 0.45f
else if status == IN_AIR:
    drag = 0.9f
else if status == ON_LAND:
    drag = landFriction
    if getControllingPassenger() instanceof Player: landFriction /= 2.0f   // the field, after reading it
v = getDeltaMovement()
setDeltaMovement(v.x * (double)drag, v.y + gravityY, v.z * (double)drag)
deltaRotation *= drag                                                 // float
if buoyancy > 0.0:
    v = getDeltaMovement()
    setDeltaMovement(v.x, (v.y + buoyancy * (getDefaultGravity() / 0.65)) * 0.75, v.z)
```

`waterLevel` is the field `getStatus` set this tick (`IN_WATER`: from `checkInWater`).
`getAirDrag()` is `1.0f` (lines 1618–1622) and is not used by this tick. On the server
`getControllingPassenger() instanceof Player` is false whenever this runs (a player
makes the server not authoritative), so the halving is the client's.

### 4.5 `controlBoat()` (lines 1623–1737; **the client's**, for the porter's model of the
client and for the check of its moves)

If `!isVehicle()` return. `f = 0.0f`. `inputLeft` → `deltaRotation -= 1.0f`;
`inputRight` → `deltaRotation += 1.0f`; if `inputRight != inputLeft && !inputUp &&
!inputDown` → `f += 0.005f`. `setYRot(getYRot() + deltaRotation)`. `inputUp` → `f +=
0.04f`; `inputDown` → `f -= 0.005f`. `setDeltaMovement(getDeltaMovement().add((double)(
Mth.sin((double)(-getYRot() * 0.017453292f)) * f), 0.0, (double)(Mth.cos((double)(
getYRot() * 0.017453292f)) * f)))`. `setPaddleState((inputRight && !inputLeft) ||
inputUp, (inputLeft && !inputRight) || inputUp)`. The client then sends
`ServerboundPaddleBoatPacket(getPaddleState(0), getPaddleState(1))`.

### 4.6 Seats: passengers, positions, rotation, dismounting

- `getMaxPassengers()` 2 (lines 2184–2188); chest boats and chest rafts 1
  (AbstractChestBoat.txt 30–34).
- `canAddPassenger(e)` (lines 2167–2183): `getPassengers().size() < getMaxPassengers()
  && !isEyeInFluid(#minecraft:water)` (the **boat's** eye, at its full height 0.5625:
  nobody boards a sunken boat). Nothing about the passenger is checked here.
- `getPassengerAttachmentPoint(e, dimensions, scale)` (lines 171–216): `x =
  getSinglePassengerXOffset()` (0.0f; chest boats 0.15f, AbstractChestBoat.txt 25–29);
  if more than one passenger: `x = indexOf(e) == 0 ? 0.2f : -0.6f`, and `x += 0.2f` if
  `e` is an `Animal`. Return `new Vec3(0.0, rideHeight(dimensions), (double)x).yRot(
  -getYRot() * 0.017453292f)`. `rideHeight`: `Boat`, `ChestBoat` `(double)(height /
  3.0f)` = 0.1875; `Raft`, `ChestRaft` `(double)(height * 0.8888889f)` = 0.5
  (Boat.txt, Raft.txt lines 11–18). (So the "x" is along the boat's length, the
  vector's z before the turn.) `Entity.positionRider` (Entity.txt 6815–6850) puts the
  passenger at `position() + that - passenger.getVehicleAttachmentPoint(this)`
  (section 1; the player's vehicle attachment is in `EntityType`, not read).
- `positionRider(e, move)` (lines 1756–1789): `Entity.positionRider`; then, unless `e`
  is in `#minecraft:can_turn_in_boats` (`breeze` only): if
  `e.isLocalInstanceAuthoritative()`: `e.setYRot(e.getYRot() + deltaRotation)`,
  `e.setYHeadRot(e.getYHeadRot() + deltaRotation)` (for a player on the server
  `isLocalInstanceAuthoritative` is false, **Inferred** as in finding 1); then
  `clampRotation(e)`.
- `clampRotation(e)` (lines 1915–1966): `body = calculatePassengerBodyYRot(e)` (lines
  1967–1993: an `Animal` in a full boat faces `getYRot() + (e.getId() % 2 == 0 ? 90.0f :
  270.0f)`, anyone else `getYRot()`); `e.setYBodyRot(body)`; a `Mob`: `setYRot(body)`,
  `clampHeadRotationToBody()`, return `wrapDegrees(body - e.getYRot())`. Others (the
  player): `d = wrapDegrees(e.getYRot() - body)`; `c = clamp(d, -105.0f, 105.0f)`;
  `e.setYRot(e.getYRot() + c - d)`; return `c - d`. So the server keeps a rider's yaw
  within 105 degrees of the boat's, as it shows the rider to others.
  `onPassengerTurned(e)` (lines 1994–2007): `e.yRotO += clampRotation(e)`.
- `getDismountLocationForPassenger(p)` (lines 1790–1914): `v =
  getCollisionHorizontalEscapeVector((double)(getBbWidth() * Mth.SQRT_OF_TWO),
  (double)p.getBbWidth(), p.getYRot())` (Entity.txt: `d = (w1 + w2 +
  9.999999747378752E-6) / 2.0`; `s = -Mth.sin((double)(yRot * 0.017453292f))`, `c =
  Mth.cos(…)`; `m = max(|s|, |c|)`; `Vec3((double)s * d / (double)m, 0, (double)c * d /
  (double)m)`); `x = getX() + v.x`, `z = getZ() + v.z`; `at = BlockPos.containing(x,
  getBoundingBox().maxY, z)`, `below = at.below()`. If `!level.isWaterAt(below)`:
  candidates, in order, `(x, at.y + h, z)` if `DismountHelper.isBlockFloorValid(h =
  level.getBlockFloorHeight(at))`, then `(x, below.y + h2, z)` likewise for `below`;
  for each pose of `p.getDismountPoses()`, for each candidate: if
  `DismountHelper.canDismountTo(level, candidate, p, pose)`: `p.setPose(pose)`, return
  it. Otherwise (water below, or nothing fits) `Entity`'s: `(getX(),
  getBoundingBox().maxY, getZ())` (section 1, DismountHelper).
- `interact(player, hand, at)` (lines 2024–2057): `VehicleEntity`'s (= `Entity`'s; the
  lead's use, section 7) unless that is `PASS`; then if `!player.isSecondaryUseActive()
  && outOfControlTicks < 60.0f`: on the server `player.startRiding(this)` and `SUCCESS`
  if it succeeded, else fall through to `PASS`. A sneaking player does not board.
  **Chest boats** (AbstractChestBoat.txt 88–136): after `AbstractBoat`'s, if `PASS`
  and (`!canAddPassenger(player)` or the player is sneaking): `interactWithContainerVehicle(
  player)` (opens the 27-slot container; `ContainerEntity`, named only), and if that
  consumed the action: `gameEvent(CONTAINER_OPEN, player)` and
  `PiglinAi.angerNearbyPiglins(level, player, true)` (no piglins in N5).

### 4.7 Bubble columns

**`onAboveBubbleColumn(down, pos)`** (lines 217–296; called from the bubble column
block's `entityInside`, **Inferred**: `BubbleColumnBlock` not read, when the block
above the column is not water): on a server level: `isAboveBubbleColumn = true`,
`bubbleColumnDirectionIsDown = down`, and if `getBubbleTime() == 0`: `setBubbleTime(60)`.
Then, on both sides: if `!isUnderWater()` and `random.nextInt(100) == 0`:
`level.playLocalSound(x, y, z, getSwimSplashSound(), source, 1.0f, 0.8f + 0.4f *
random.nextFloat(), false)` and `level.addParticle(SPLASH, x + random.nextFloat(), y +
0.7, z + random.nextFloat(), 0, 0, 0)` (both nothing on a server level, **Inferred**,
but **the draws happen**: the argument expressions are evaluated), then
`gameEvent(GameEvent.SPLASH, getControllingPassenger())`. It does not call
`Entity.onAboveBubbleColumn`. `onInsideBubbleColumn` is `Entity`'s (Entity.txt
8135–8166: only where `canSimulateMovement()`; `y = down ? max(-0.3, v.y - 0.03) :
min(0.7, v.y + 0.06)`; `resetFallDistance`).

**`tickBubbleColumn()`** (lines 659–761), on the server:
1. if `!isAboveBubbleColumn`: `setBubbleTime(0)`.
2. `t = getBubbleTime()`; if `t > 0`: `t--`, `setBubbleTime(t)`; if `60 - t - 1 > 0 &&
   t == 0`: `setBubbleTime(0)`; if `down`: `ejectPassengers()` and
   `level.broadcastEntityEvent(this, 71)`; else `broadcastEntityEvent(this, 72)`;
   then `handleBubbleColumnEffect(down)`; and (inside `t > 0`) `isAboveBubbleColumn =
   false`.
`handleBubbleColumnEffect(down)` (lines 782–809): only where `canSimulateMovement()`
(for a boat, `MoveSimulationType.AUTHORITATIVE_SIDE`, so `isLocalInstanceAuthoritative`;
Entity.txt 10369–10410 and `Entity$1`): `down` → motion `+ (0, -0.7, 0)`; else motion
`y = 0.6`. So a driven boat gets the effect from the client, which `handleEntityEvent`
71/72 (lines 762–781) gives it; an undriven boat gets it from the server. The client
counts nothing: `DATA_ID_BUBBLE_TIME` animates its rocking (`bubbleMultiplier`,
`bubbleAngle`, the client branch at the start of `tickBubbleColumn`).
Timing: the column sets 60 in a tick's `applyEffectsFromBlocks`, `tickBubbleColumn`
takes it to 59 in that tick, and the effect fires in the 60th tick the boat stays
above the column; the next tick sets 60 again. A tick away from it resets the count.
The flag is cleared only inside `t > 0`, which always holds while it is set.

### 4.8 Synched data, save, constants

Accessors in order (ids continue `Entity`'s 0–7: shared flags `BYTE`, air `INT`, custom
name `OPTIONAL_COMPONENT`, name visible `BOOLEAN`, silent `BOOLEAN`, no gravity
`BOOLEAN`, pose `POSE`, frozen ticks `INT`):

| id | field | serializer | default |
|----|-------|------------|---------|
| 8 | `VehicleEntity.DATA_ID_HURT` | `INT` | 0 |
| 9 | `VehicleEntity.DATA_ID_HURTDIR` | `INT` | 1 |
| 10 | `VehicleEntity.DATA_ID_DAMAGE` | `FLOAT` | 0.0 |
| 11 | `AbstractBoat.DATA_ID_PADDLE_LEFT` | `BOOLEAN` | false |
| 12 | `AbstractBoat.DATA_ID_PADDLE_RIGHT` | `BOOLEAN` | false |
| 13 | `AbstractBoat.DATA_ID_BUBBLE_TIME` | `INT` | 0 |

(VehicleEntity.txt 153–174, 250–266; AbstractBoat.txt 100–124, 2296–2311. Chest boats
define nothing more.)

Saved (`addAdditionalSaveData`, lines 2008–2023): the leash only
(`writeLeashData`/`readLeashData`, section 7); chest boats also
`addChestVehicleSaveData` (`Items`, `LootTable`, `LootTableSeed`; `ContainerEntity`).
Not saved: status, `outOfControlTicks`, paddle positions, bubble time, damage, hurt
time (all start at their defaults on load, as on an arrival in Clustine).

Numbers: gravity 0.04 (`getDefaultGravity`); the drags of 4.4; air drag 1.0f (unused);
`getMovementEmission()` `EVENTS` (game events, no step sounds; lines 95–99);
`isPickable()` `!isRemoved()`; `isPushable()` true; `getMotionDirection()` the facing
turned clockwise; the client interpolates over 3 ticks (`LinearInterpolationHandler`,
lines 355–361; **Client**). `getPickResult` and the drop are `new ItemStack(dropItem)`,
the boat's item (supplied per type at registration; **Inferred**: the item of the same
name, `EntityType`/`Items` not read). `getLeashOffset()` (lines 880–897): `Vec3(0.0,
(double)(0.88f * getBbHeight()), (double)(0.64f * getBbWidth()))` = (0, 0.495000004…,
0.879999995…); quad offsets `Leashable.createQuadLeashOffsets(this, 0.0, 0.64, 0.382,
0.88)` (section 7). `modifyPassengerFluidInteractionBox(box)` (lines 2256–2295): a
passenger's fluid box is cut to above the boat's top unless the boat is under water
(`null`, no fluid at all, if the boat's top is at or above the passenger's): a rider
in a floating boat is not "in water".

### 4.9 Collisions and pushing

- `canCollideWith(e)` = `canVehicleCollide(this, e)` (lines 125–149):
  `(e.canBeCollidedWith(this) || e.isPushable()) && !isPassengerOfSameVehicle(e)`.
  Minecarts use the same static (3.2).
- `canBeCollidedWith(e)` true; `isPushable()` true.
- `push(e)` (lines 297–326): if `e` is a boat, `VehicleEntity.push(e)` (= `Entity.push`)
  only if `e.box.minY < this.box.maxY`; any other entity only if `e.box.minY <=
  this.box.minY`. `Entity.push(e)` (Entity.txt 5187–5283): return if passengers of the
  same vehicle, or either has `noPhysics`; `dx = e.x - x`, `dz = e.z - z`, `m =
  Mth.absMax(dx, dz)`; if `m >= 0.009999999776482582`: `m = sqrt(m)`; `dx /= m`, `dz /=
  m`; `k = min(1.0 / m, 1.0)`; `dx *= k`, `dz *= k`; `dx *= 0.05000000074505806`, `dz
  *= 0.05000000074505806`; if `!this.isVehicle() && this.isPushable()`: `this.push(-dx,
  0, -dz)`; if `!e.isVehicle() && e.isPushable()`: `e.push(dx, 0, dz)`.
  `push(x, y, z)` (5306–5321): if all finite, motion `+= (x, y, z)` and `needsSync =
  true`. A ridden boat is never pushed (it is a vehicle); it pushes others.
- Others push boats through their own `push` (players' `LivingEntity.pushEntities`,
  minecarts): the boat is moved only while nobody rides it.

### 4.10 Breaking (what the boat adds to `VehicleEntity.hurtServer`)

`VehicleEntity.hurtServer` (VehicleEntity.txt 22–97; detailed in 3.14):
`hurtDir = -hurtDir`, `hurtTime = 10`, `markHurt`, `damage += amount * 10.0f`, game
event `ENTITY_DAMAGE`; a player with `instabuild` (creative) → `discard()` (no drop,
since `shouldSourceDestroy` is false); otherwise `damage > 40.0f` → `destroy` (`kill`
and, with `entityDrops`, the boat item named as the boat). The boat's own part: the
`damage` decays by 1 a tick (4.2 step 4), `animateHurt` is the client's.
`remove(reason)` (lines 2058–2076): on the server, if `reason.shouldDestroy()`
(`KILLED` and `DISCARDED` both do, Entity$RemovalReason.txt 90–102) and leashed,
`dropLeash()`. **Chest boats** (AbstractChestBoat.txt 55–87): `destroy` also calls
`chestVehicleDestroyed`; `remove` with `shouldDestroy` calls `Containers.dropContents(
level, this, this)` before `AbstractBoat.remove`: **a creative player's discard of a
chest boat spills its contents** (no boat item).

### 4.11 Rafts and chest boats

`Raft`/`ChestRaft` override only `rideHeight` (4.6). `AbstractChestBoat`: 27 slots,
loot table and seed, one passenger, the 0.15 seat offset, container interaction and
drops (named; `ContainerEntity` is 3.18). Same size, data and tick as boats.

---------------------------------------------------------------------------------------

## 5. Armour stands (and the two other things a player can stand or sit by: `Mannequin`, `Cushion`)

Line ranges are of `ArmorStand.txt`, `ArmorStandItem.txt`, `Cushion.txt`,
`BlockAttachedEntity.txt` and `Mannequin.txt` in the javap dump. Damage-type tags are
from `generated/data/minecraft/tags/damage_type/`.

### 5.0 Findings of this section

1. **`Cushion` is a seat, new in 26.3, and stable (not behind a feature flag).** It is a
   `BlockAttachedEntity` (the base of hanging entities, section 6) that lies on top of a
   block, 1.0 × 0.25, coloured (`DATA_COLOR`, a `DyeColor`), made from three wool slabs
   (`generated/data/minecraft/recipe/white_cushion.json`), sixteen items
   `minecraft:<colour>_cushion` (stack 16, component `minecraft:cushion/color`, tag
   `#minecraft:cushions`), in no experimental datapack. A player right-clicks it and
   **sits on it** (`Player.startRiding(cushion)`), and gets up by sneaking. It breaks
   in one hit, drops itself unless the attacker is creative, and drops when the block
   under it goes (checked every 101 ticks). A creative player meets it at once in the
   inventory and will try sitting on it: **it belongs in N5**, as the simplest vehicle
   there is (it never moves) and as a second user of `BlockAttachedEntity` beside the
   hanging entities. Its placing item's class is not in the dump (see "Not read").
2. **`Mannequin` is a player-shaped `LivingEntity`** (`extends Avatar`, a class shared
   with players): a skin (`ResolvableProfile`), hidden model layers, a main hand, a
   pose among standing/crouching/swimming/fall flying/sleeping, an `immovable` flag and
   a text label under it ("NPC", `entity.minecraft.mannequin.label`). There is **no item
   for it** (the item registry has none, and no spawn egg): it is made only by
   `/summon` or a structure/command. It needs LivingEntity's whole physics, health,
   equipment and attributes. **It does not belong in N5**; it belongs with living
   entities (N7), and until then a world that has one keeps it as an entity of unknown
   behaviour (its NBT kept, ADR-0028 section 2), shown to clients with its data.
3. **An armour stand is a `LivingEntity`.** Its gravity, drag, fall, water and lava,
   health, fire and equipment are LivingEntity's, which is not in the dump. N5 needs
   at least `LivingEntity.tick`/`aiStep`/`travel` (the plain non-flying, non-swimming
   travel with gravity 0.08 and `0.91`-style friction **Inferred**), `hurt` bookkeeping,
   equipment and its `SetEquipment` packets, attributes and their pairing packet
   (`UpdateAttributes`), `dropAllDeathLoot`. That is a large read for one entity; the
   plan should decide whether N5 ports LivingEntity's movement now (it is N7's anyway)
   or gives armour stands a reduced form. ServerEntity sends a LivingEntity three
   pairing packets beyond `AddEntity`/`SetEntityData`: `UpdateAttributes` (if any
   syncable attribute), `SetEquipment` (if any slot is not empty), then the passenger
   packets (ServerEntity.txt lines 869–1000).
4. **Armour stands break on two hits within 5 ticks of *game time***
   (`ServerLevel.getGameTime() - lastHit > 5` makes a hit a first hit). `lastHit` is
   not saved and is compared with the world's game time, so Clustine needs one world
   game time that every region agrees on (it has one for the day cycle **Inferred**),
   not a region's own tick count.
5. **The interact path in 26.3 is one method, `interact(Player, InteractionHand,
   Vec3 location)`**; the old `interactAt` is gone. The armour stand reads the click's
   height from `location` (relative to the entity's feet **Inferred**: the server's
   packet handler computes it, section 2).
6. **Placing an armour stand makes a level random draw that is then thrown away**
   (**Inferred**: `EntityType.create` sets a random yaw, `snapTo` then overwrites it with
   the snapped one). Not reproducible in Clustine and not visible; name it and draw
   nothing.

### 5.1 Placing: `ArmorStandItem.useOn(UseOnContext)` (ArmorStandItem.txt lines 10–135)

1. `face = ctx.getClickedFace()`; if `face == DOWN` → `FAIL` (nothing else read).
2. `place = new BlockPlaceContext(ctx)`; `pos = place.getClickedPos()` (the clicked
   block if it is replaceable by this placing, else the block beside the clicked face:
   **Inferred** from `BlockPlaceContext`, which the repo's block placing already
   follows).
3. `bottom = Vec3.atBottomCenterOf(pos)` = `(pos.x + 0.5, pos.y, pos.z + 0.5)`;
   `box = ARMOR_STAND.getDimensions().makeBoundingBox(bottom)`: width 0.5, height
   1.975 (`0x3ffccccd` widened), so `x ± 0.25`, `y .. y + 1.975`, `z ± 0.25`.
4. If `!level.noCollision(null, box)` (block collisions and the world border, and
   entities that collide like blocks: boats, shulkers, minecarts are not among them
   **Inferred**, `Level.noCollision` not read) or `!level.getEntities(null,
   box).isEmpty()` (any entity whose box intersects, with the default predicate, which
   leaves out spectators **Inferred**) → `FAIL`.
5. Server only (lines 106–259):
   - `config = EntityType.createDefaultStackConfig(level, stack, player)`: applies the
     item's `entity_data` and custom name to the entity (**Inferred**, not read).
   - `stand = ARMOR_STAND.create(level, config, pos, SPAWN_ITEM_USE, true, true)`; null
     → `FAIL`. With `(true, true)` the position is `pos`'s bottom centre raised by
     `EntityType.getYOffset` (the collision under the box, looking up to two blocks
     down, so a stand placed on a slab or a carpet stands on it) and **a random yaw is
     drawn**, `Mth.wrapDegrees(level.random.nextFloat() * 360.0f)` (both
     **Inferred**: `EntityType.create` is not in the dump).
   - `yaw = (float) Mth.floor((Mth.wrapDegrees(ctx.getRotation() - 180.0f) + 22.5f) /
     45.0f) * 45.0f`, all in `float` (`Mth.floor(float)`, then `i2f`, then `* 45f`):
     the player's yaw turned to face them, snapped to the nearest 45°.
     `ctx.getRotation()` is the player's `yRot` (**Inferred**).
   - `stand.snapTo(stand.getX(), stand.getY(), stand.getZ(), yaw, 0.0f)` (position
     kept, rotation set; `snapTo` also sets the old position and rotations).
   - `level.addFreshEntityWithPassengers(stand)`.
   - `level.playSound(null, x, y, z, ARMOR_STAND_PLACE, BLOCKS, 0.75f, 0.8f)` (no
     player excluded: the placer hears it from the server too).
   - `stand.gameEvent(ENTITY_PLACE, player)`.
6. `stack.shrink(1)` (both sides; a creative player's count is put back by
   `ServerPlayerGameMode.useItemOn`, **Inferred**, as for blocks) → `SUCCESS`.

The body, head and yaw: `setYBodyRot(f)` and `setYHeadRot(f)` both set `yRotO`,
`yBodyRotO`, `yHeadRot` and `yHeadRotO` to `f` (lines 1104–1135), and
`tickHeadTurn` sets `yBodyRotO = yRotO; yBodyRot = getYRot()` (lines 1081–1092): an
armour stand's body and head always face its yaw. ServerEntity's head rotation packet
therefore follows the yaw.

### 5.2 The pose and the flags

**Pose** (`ArmorStand$ArmorStandPose`, a record of six `Rotations` (three `float`s,
x/y/z in degrees); codec fields `Head`, `Body`, `LeftArm`, `RightArm`, `LeftLeg`,
`RightLeg`, each `optionalFieldOf` with its default, ArmorStandPose.txt lines 98–130).
Defaults (static block, ArmorStand.txt lines 1734–1777; the marker and small sizes at 1778–1788):

| Part | Default (x, y, z) |
|---|---|
| head | (0, 0, 0) |
| body | (0, 0, 0) |
| left arm | (-10, 0, -10) |
| right arm | (-15, 0, 10) |
| left leg | (-1, 0, -1) |
| right leg | (1, 0, 1) |

The game never changes a pose by itself; only NBT (commands, the item's
`entity_data`) does. Clustine keeps the six as data.

**Flags** (`DATA_CLIENT_FLAGS`, a byte; `setBit` ORs or clears, lines 1203–1362):

| Bit | Meaning | Getter |
|---|---|---|
| 1 | small | `isSmall()` (also `isBaby()`, line 1154) |
| 4 | show arms | `showArms()` |
| 8 | no base plate | `showBasePlate()` is `(flags & 8) == 0` |
| 16 | marker | `isMarker()` |

(Bit 2 is unused.) `onSyncedDataUpdated(DATA_CLIENT_FLAGS)` (lines 1537–1557):
`refreshDimensions()` (the override, lines 112–131, keeps `x, y, z` and remakes the box
around them) and `blocksBuilding = !isMarker()`.

**Dimensions** (`getDefaultDimensions` → `getDimensionsMarker(isMarker())`, lines
1563–1586): marker → `EntityDimensions.fixed(0, 0)` (a box of no size at the feet);
small → `ARMOR_STAND.getDimensions().scale(0.5f).withEyeHeight(0.9875f)` (0.25 ×
0.9875); otherwise the type's 0.5 × 1.975, eye 1.7775.

**Invisible**: the stand keeps its own field `invisible` beside the shared flag (bit 5
of entity data 0): `setInvisible(b)` sets both (lines 1144–1153), and
`updateInvisibilityStatus()` (called by LivingEntity when effects change) sets the
flag back from the field (lines 1136–1143), so a potion cannot make a stand visible or
invisible.

### 5.3 Interaction: `interact(player, hand, location)` (lines 367–460)

```
stack = player.getItemInHand(hand)
if isMarker() || stack.is(NAME_TAG): return LivingEntity.interact(player, hand, location)
if player.isSpectator(): return SUCCESS
(client: SUCCESS_SERVER)
forItem = getEquipmentSlotForItem(stack)          // LivingEntity: the equippable component's slot, else MAINHAND (Inferred)
if stack.isEmpty():
    clicked = getClickedSlot(location)
    slot = isDisabled(clicked) ? forItem : clicked
    if hasItemInSlot(slot) && swapItem(player, slot, stack, hand): return SUCCESS_SERVER
else:
    if isDisabled(forItem): return FAIL
    if forItem.getType() == HAND && !showArms(): return FAIL
    if swapItem(player, forItem, stack, hand): return SUCCESS_SERVER
return LivingEntity.interact(player, hand, location)
```

**`getClickedSlot(location)`** (lines 461–572): `small = isSmall()`; `y = location.y /
(double)(getScale() * getAgeScale())` (the `float` product widened, then a `double`
divide; `getAgeScale()` is 0.5 for a baby, so 0.5 for a small stand, **Inferred**;
`getScale()` is the `scale` attribute, 1.0 by default **Inferred**). Then, first match
wins, each also needing the slot to hold an item:

| Slot | Condition on `y` (all `double`; the sums computed at run time) |
|---|---|
| FEET | `y >= 0.1 && y < 0.1 + (small ? 0.8 : 0.45)` |
| CHEST | `y >= 0.9 + (small ? 0.3 : 0.0) && y < 0.9 + (small ? 1.0 : 0.7)` |
| LEGS | `y >= 0.4 && y < 0.4 + (small ? 1.0 : 0.8)` |
| HEAD | `y >= 1.6` |
| OFFHAND | mainhand empty and offhand not |
| MAINHAND | otherwise |

(`>=` is `dcmpl … iflt`, `<` is `dcmpg … ifge`: a NaN `y` matches nothing.) Note the
small stand's thresholds are not halved; the click height is doubled instead.

**`isDisabled(slot)`** (lines 573–595): `(disabledSlots & (1 << slot.getFilterBit(0)))
!= 0 || (slot.getType() == HAND && !showArms())`. **`disabledSlots`** is an `int`
saved as `DisabledSlots`: bit `filterBit + 0` disables the slot outright, bit
`filterBit + 8` (`DISABLE_TAKING_OFFSET`) forbids taking from it, bit `filterBit +
16` (`DISABLE_PUTTING_OFFSET`) forbids putting into it. `EquipmentSlot.getFilterBit(o)
= o + filterFlag`, with filter flags MAINHAND 0, FEET 1, LEGS 2, CHEST 3, HEAD 4,
OFFHAND 5, BODY 6, SADDLE 7 (**Inferred**, `EquipmentSlot` not read).
`canUseSlot(slot)` (lines 201–217) is false for BODY, SADDLE and disabled slots.

**`swapItem(player, slot, inHand, hand)`** (lines 596–677):

1. `held = getItemBySlot(slot)`.
2. `held` not empty and its taking bit set → false.
3. `held` empty and its putting bit set → false.
4. `player.hasInfiniteMaterials()` (creative) and `held` empty and `inHand` not empty →
   `setItemSlot(slot, inHand.copyWithCount(1))`, true. **The creative player keeps
   their stack.**
5. `inHand` not empty and `inHand.getCount() > 1`: `held` not empty → false; else
   `setItemSlot(slot, inHand.split(1))` (the hand stack shrinks by one), true.
6. Otherwise (hand empty, or a single item): `setItemSlot(slot, inHand);
   player.setItemInHand(hand, held)`, true: a swap. A creative player taking an item
   off a stand gets it into their hand this way, and the stand's slot takes what they
   held (empty, or the one item).

`setItemSlot` goes through LivingEntity (equip sound, `SetEquipment` to viewers:
**Inferred**, not read).

### 5.4 Being hit: `hurtServer(level, source, amount)` (lines 678–852)

In order, the first rule that applies:

1. Removed → false.
2. `mobGriefing` off and the source's entity is a `Mob` → false.
3. Source in `#bypasses_invulnerability` (`out_of_world`, `generic_kill`) →
   `kill(level, source.getEntity())`, false. (No drops: `/kill` and the void.)
4. `isInvulnerableTo(level, source)` or the `invisible` field or a marker → false.
5. `#is_explosion` (`fireworks`, `explosion`, `player_explosion`,
   `bad_respawn_point`) → `brokenByAnything` (equipment drops, not the stand), `kill`,
   false. (Explosions are N6; `ignoreExplosion`, lines 1175–1185: an explosion that
   affects block-like entities ignores the stand only if it is invisible; one that
   does not affects none.)
6. `#ignites_armor_stands` (`in_fire`, `campfire`): on fire → `causeDamage(0.15f)`;
   else `igniteForSeconds(5.0f)`; false.
7. `#burns_armor_stands` (`on_fire`) and `getHealth() > 0.5f` → `causeDamage(4.0f)`,
   false.
8. `canBreak = #can_break_armor_stand` (`player_explosion`, `#is_player_attack` =
   `player_attack`, `spear`, `mace_smash`); `alwaysKills =
   #always_kills_armor_stands` (`arrow`, `trident`, `fireball`, `wither_skull`,
   `wind_charge`). Neither → false.
9. The source's entity is a player without `abilities.mayBuild` (adventure mode) →
   false.
10. `source.isCreativePlayer()` → `playBrokenSound()`, `showBreakingParticles()`,
    `kill(level, attacker)`, **true. Nothing drops, neither the stand nor what it
    wears.**
11. `now = level.getGameTime()`. If `now - lastHit > 5` and not `alwaysKills`: **first
    hit**: `level.broadcastEntityEvent(this, 32)`, `gameEvent(ENTITY_DAMAGE,
    attacker)`, `lastHit = now`. Otherwise (a second hit within 5 game ticks, or an
    arrow and the like): `brokenByPlayer(level, source)`, `showBreakingParticles()`,
    `kill(level, attacker)`. True either way.

`lastHit` is a `long`, 0 at creation and not saved; so in a world younger than 6 ticks
the first hit breaks the stand. Entity event 32 makes the client play
`entity.armor_stand.hit` (volume 0.3, pitch 1) and set its own `lastHit` for the
wobble (`handleEntityEvent`, lines 853–887, **Client**). The stand never takes
LivingEntity's damage path: no knockback, no hurt time, no health loss from a hit.

**`causeDamage(level, source, amount)`** (lines 963–995): `h = getHealth() - amount`
(`float`); `h <= 0.5f` → `brokenByAnything`, `kill`; else `setHealth(h)`,
`gameEvent(ENTITY_DAMAGE, attacker)`. (Health starts at the max-health attribute's
default, 20, **Inferred**; LivingEntity not read.)

**`brokenByPlayer(level, source)`** (lines 996–1020): `stack = new
ItemStack(ARMOR_STAND)` with `CUSTOM_NAME` set to the stand's custom name (null
clears it); `Block.popResource(level, blockPosition(), stack)` (N4 brief section 8.4:
three level random draws for its offset, and the item's own motion draws); then
`brokenByAnything`.

**`brokenByAnything(level, source)`** (lines 1021–1061): `playBrokenSound()`;
`dropAllDeathLoot(level, source)` (LivingEntity: the loot table
`entities/armor_stand` is empty, `{"type":"minecraft:entity","random_sequence":
"minecraft:entities/armor_stand"}`, and it may draw nothing; not read); then for each
slot in `EquipmentSlot.VALUES` order (MAINHAND, OFFHAND, FEET, LEGS, CHEST, HEAD,
BODY, SADDLE **Inferred**): `old = equipment.set(slot, EMPTY)`; if not empty and it
does not have `prevent_equipment_drop` (curse of vanishing) →
`Block.popResource(level, blockPosition().above(), old)`.

**`playBrokenSound()`** (lines 1062–1080): `level.playSound(null, x, y, z,
ARMOR_STAND_BREAK, getSoundSource(), 1.0f, 1.0f)`. **`showBreakingParticles()`**
(lines 920–962): `sendParticles(BLOCK oak_planks, x, getY(0.6666666666666666), z, 10,
width / 4f, height / 4f, width / 4f, 0.05)` (spreads `float` divides widened).

**`kill(level, attacker)`** (lines 1160–1174): `remove(KILLED)`;
`gameEvent(ENTITY_DIE, attacker != null ? attacker : this)`. No death animation, no
`EntityEvent` 3: the stand is gone at once.

Other rules: `attackable()` false (line 1558: mobs do not target it);
`skipAttackInteraction(e)` (lines 1484–1503) is true for a player who may not interact
at its block (`level.mayInteract`, spawn protection), so their hit does nothing;
`isPickable()` is LivingEntity's and not a marker; `thunderHit` does nothing;
`isAffectedByPotions()` false; `getPickResult()` an armour stand item; hurt and death
sounds `armor_stand.hit`/`armor_stand.break`, fall sounds `armor_stand.fall` (both).

The packet path for a hit (`Attack` → `player.attack(stand)`)
is section 2.8's. **Inferred**: a creative player's attack reaches `hurtServer` through
`Player.attack` with a `player_attack` source for which `isCreativePlayer()` is true.

### 5.5 Physics

All of it is LivingEntity's (`tick` → `aiStep` → `travel`), with these overrides:

- **`hasPhysics()`** = `!isMarker() && !isNoGravity()` (lines 132–144).
- **`travel(v)`** returns at once without physics (lines 1093–1103): a marker or a
  no-gravity stand never moves by itself (not even by its motion).
- **`isEffectiveAi()`** = LivingEntity's and `hasPhysics()` (lines 145–157).
- **`readAdditionalSaveData` sets `noPhysics = !hasPhysics()`** (lines 268–325, at
  83–96), and only there: a stand loaded as a marker or with `NoGravity` does not
  collide; one made a marker later (a command changing the flags) keeps colliding
  until reloaded.
- **Attributes**: LivingEntity's, with `step_height` 0 (`createAttributes`, lines
  104–111).
- **Pushing**: `isPushable()` false (no entity pushes it), `doPush` empty (lines
  326–334). **`pushEntities()`** (lines 335–366, called from LivingEntity's `aiStep`
  **Inferred**): every entity in its box that is an `AbstractMinecart` with
  `isRideable()` and `distanceToSqr(stand) <= 0.2` is pushed by the stand
  (`cart.push(stand)`, Entity's push between two entities, section 1/3). So a stand
  standing on a rail pushes a rideable cart that runs into it; nothing else.
- **Piston**: marker → `IGNORE_ENTITY`, else LivingEntity's (`NORMAL`) (lines
  1186–1196).
- **Block triggers**: a marker ignores them (`isIgnoringBlockTriggers`, lines
  1197–1202: pressure plates, tripwires).
- A stand **falls, floats in water, burns in lava and fire and takes fall damage** by
  LivingEntity's rules (fire through 5.4's rules 6 and 7; a fall's damage source is
  not in any of the tags of 5.4, so falls do nothing, **Inferred**).

**Inferred** for the plan: LivingEntity's gravity is the `gravity` attribute, 0.08;
air drag `0.98f` on y; horizontal friction `blockFriction * 0.91f`; on ground a stand
does not slide. An armour stand at rest makes no change each tick once it is on the
ground (unlike an item, N4 brief finding 5), but this must be read in LivingEntity.

### 5.6 Synched data, in order

Entity's (0–7, section 8), LivingEntity's (8–14, **not read**; ids and serializers as
SteelMC's extraction lists them, for orientation only: 8 `DATA_LIVING_ENTITY_FLAGS`
byte, 9 `DATA_HEALTH_ID` float (1.0 until the max health is set), 10
`DATA_EFFECT_PARTICLES` particles list, 11 `DATA_EFFECT_AMBIENCE_ID` boolean, 12
`DATA_ARROW_COUNT_ID` int, 13 `DATA_STINGER_COUNT_ID` int, 14 `SLEEPING_POS_ID`
optional block pos), then ArmorStand's own, defined in this order in the static block
(lines 1789–1816) and `defineSynchedData` (lines 158–200):

| Id | Accessor | Serializer | Default |
|---|---|---|---|
| 15 | `DATA_CLIENT_FLAGS` | `BYTE` | 0 |
| 16 | `DATA_HEAD_POSE` | `ROTATIONS` | (0, 0, 0) |
| 17 | `DATA_BODY_POSE` | `ROTATIONS` | (0, 0, 0) |
| 18 | `DATA_LEFT_ARM_POSE` | `ROTATIONS` | (-10, 0, -10) |
| 19 | `DATA_RIGHT_ARM_POSE` | `ROTATIONS` | (-15, 0, 10) |
| 20 | `DATA_LEFT_LEG_POSE` | `ROTATIONS` | (-1, 0, -1) |
| 21 | `DATA_RIGHT_LEG_POSE` | `ROTATIONS` | (1, 0, 1) |

(The ids 15–21 follow from LivingEntity's seven; that count is SteelMC's, to confirm
in `LivingEntity`.) `ROTATIONS` writes three `float`s. Entity type `armor_stand`, id
5; 0.5 × 1.975, eye 1.7775; tracking range 10 chunks, update interval 3, deltas
tracked (`entity_types.rs`). Pairing (ServerEntity.txt lines 869–1000): `AddEntity`
(data 0), `SetEntityData` of the non-default values, `UpdateAttributes` if any
attribute is syncable, `SetEquipment` with every non-empty slot.

### 5.7 What is saved (`addAdditionalSaveData`, lines 218–267)

LivingEntity's (health, equipment, attributes, effects, `Brain`… not read), then:
`Invisible` (the field), `Small`, `ShowArms`, `DisabledSlots` (int), `NoBasePlate`,
`Marker` **only when true**, and `Pose` (the record, each part written only when it
differs from its default, **Inferred** from `optionalFieldOf` with a default). Read
back with defaults false/0; `Pose` absent keeps the defaults; `noPhysics` set from the
read flags.

---

### 5.8 `Cushion` (Cushion.txt; its base `BlockAttachedEntity`, BlockAttachedEntity.txt)

**What it is**: a seat lying on a block. Type `cushion`, id 33, 1.0 × 0.25, eye 0.2125;
tracking range 10, update interval `Integer.MAX_VALUE`, deltas not tracked
(`entity_types.rs`): its position is sent once, at pairing. Passenger attachment
`(0, 0.25, 0)` (SteelMC's extraction of `EntityType`, orientation; read in
`EntityType` with the riding of section 1).

**Position and box.** Unlike the other block-attached entities its position is free:
`setPos(x, y, z)` (lines 288–300) sets the raw position first, then
`BlockAttachedEntity.setPos`, which sets `pos = BlockPos.containing(x, y, z)`,
recomputes the box and sets `needsSync` (BlockAttachedEntity.txt lines 325–338). Its
box is Entity's `makeBoundingBox()` from position and size (`recalculateBoundingBox`,
lines 504–510): `x ± 0.5`, `y .. y + 0.25`, `z ± 0.5`.

**Tick** (BlockAttachedEntity.txt lines 34–73; it does **not** call `Entity.tick`,
so no `baseTick`, no gravity, no fluid pushing, no fire ticks): server only:
`checkBelowWorld()`; `if (ticksSinceLastCheck++ >= 100)` (the *old* value is compared:
`getfield; dup_x1; iconst_1; iadd; putfield`, then `bipush 100; if_icmplt`) →
`ticksSinceLastCheck = 0; tickAtCheckInterval(); if !isRemoved() && !survives():
discard(); dropItem(level, null)`. So the check runs on the 101st tick after the
entity is made or loaded and every 101 ticks after (`ticksSinceLastCheck` starts at 0
and is not saved). This is BlockAttachedEntity's, so hanging entities share it
(section 6).

**`tickAtCheckInterval()`** (lines 209–251): `fs = level.getBlockState(blockPosition())
.getFluidState()`; if `collidedWithFluid(fs, blockPosition(), position(),
position())`: `fs.entityInside(level, blockPosition(), this, insideEffectCollector)`
and `insideEffectCollector.applyAndClear(this)` (water puts out fire, lava sets it
alight: N4 brief section 1.9); then `destroyIfInFire(level)`.

**`destroyIfInFire(level)`** (lines 253–271, 638–656): if not removed: for the blocks
in `getBoundingBox().nextDeflated()` whose state is in `#minecraft:fire`, the first
found: `hurtServer(level, damageSources().inFire(), 1.0f)` and stop.

**`survives()`** = `wouldSurviveAt(level, getBoundingBox())` (lines 495–502):

- **`wouldSurviveAt(level, box)`** = `hasAnchorBelow(level, box) &&
  !isCoveredBySuffocatingBlocks(level, box)` (lines 367–381).
- **`hasAnchorBelow`** (lines 382–415, 616–636): `probe = AABB(box.minX, box.minY -
  0.015625, box.minZ, Math.nextDown(box.maxX), box.minY, Math.nextDown(box.maxZ))
  .expandTowards(0, -0.125, 0)` (so `minY - 0.140625 .. minY`); for each block in
  `level.findBlocksIn(probe)`: `shape = state.getShape(level, pos)` (the **outline**
  shape, not the collision shape); if not empty and `shape.bounds().move(pos)
  .intersects(probe)` → anchored (stop). The result is whether one was found
  (**Inferred**: `BlockMatcher.forEachUntil` returns true on `ABORT`). So a cushion
  is held by any block whose outline's bounding box reaches up to within 0.140625 of
  its underside: a full block, a slab, a carpet, a fence; not air or a block with an
  empty outline.
- **`isCoveredBySuffocatingBlocks`** (lines 468–493): true iff **every** position in
  `BlockPos.betweenClosed(box.nextDeflated())` is `isSuffocating`. With the cushion's
  box inside one block, that is the block it lies in.
- **`canBePlacedAt(level, box)`** = `wouldSurviveAt(level, box) &&
  !isAnchorBuried(level, box)` (lines 352–365), for the placing item:
  **`isAnchorBuried`** (lines 417–466): `slab = AABB(minX, minY, minZ, maxX, minY +
  0.015625, maxZ).nextDeflated()`; for each block collision shape `c` in
  `level.getBlockCollisions(null, slab)`: if `Shapes.join(Shapes.create(slab), c,
  ONLY_FIRST).isEmpty()` (one shape covers the whole slab) → buried.

**Interaction** (`interact`, lines 150–176): if `!player.isSecondaryUseActive()` (not
sneaking) and `!isVehicle()` (nobody sits on it): server: `player.startRiding(this)`;
on success `playSound(CUSHION_SIT, 1, 1)` → `SUCCESS_SERVER`; on failure (or client)
→ `CONSUME`. Sneaking, or occupied → `PASS`. One rider only, by this rule.
**`removePassenger(p)`** (lines 178–195): after Entity's, server-side and the cushion
not removed (`getRemovalReason() == null`) → `playSound(CUSHION_GET_UP, 1, 1)`.

**Being hit** (lines 78–148): `hurtServer` returns false if the attacker is a player
who `!mayBuild()` (adventure) or `!level.mayInteract(player, pos)` (spawn
protection); otherwise BlockAttachedEntity's (BlockAttachedEntity.txt lines 154–191):
invulnerable to the source (`isInvulnerableToBase`) → false; `mobGriefing` off and a
mob attacker → false; not removed → `kill(level, attacker)`, `markHurt()`,
`dropItem(level, attacker)`; true. **One hit breaks it.** `kill` (lines 122–137):
`onKilled()` (nothing), `remove(KILLED)`, `gameEvent(ENTITY_DIE, attacker or
this)`. A player attacking it reaches `hurtServer` by `skipAttackInteraction`
(BlockAttachedEntity.txt lines 86–112): a player who may not interact at `pos` is
skipped; otherwise `hurtOrSimulate(playerAttack(player), 0.0f)` and the attack is
skipped (true), so the player's normal attack does not run as well.

**Moving it breaks it** (BlockAttachedEntity.txt lines 215–278): `move(type, v)` with
`v.lengthSqr() > 0`, and `push(dx, dy, dz)` with a non-zero vector, on the server and
not removed → `kill(level)` and `dropItem(level, null)`. A piston (`move` with
`PISTON`) or an explosion's push therefore breaks it. `ignoreExplosion` (lines
193–213): an explosion from an entity in water ignores it; one that does not affect
block-like entities ignores it; else Entity's.

**Lightning** (`thunderHit`, lines 273–286): not removed → `kill(level, bolt)`,
`dropItem(level, bolt)`.

**`dropItem(level, breaker)`** (lines 35–76): `playSound(CUSHION_BREAK, 1, 1)`;
`showBreakingParticles()` (`BLOCK` particles of the matching wool, as the armour
stand's, lines 302–350); `entityDrops` game rule false → stop; breaker is a player with
`hasInfiniteMaterials()` (creative) → **no item**; else `spawnAtLocation(level,
stack)` with `stack` = the cushion item of its colour with its custom name
(`getCushionItemStackWithData`, lines 596–614); a lightning breaker makes the item
invulnerable for 20 ticks. `discard()` from the tick's survival check passes no
breaker, so it always drops.

**Data**: Entity's 0–7, then one of its own (`defineSynchedData`, lines 512–519;
static, lines 658–666): **8 `DATA_COLOR`, serializer `DYE_COLOR`, default `WHITE`**.
Saved: BlockAttachedEntity's `block_pos` (an `int` array; on read, a `block_pos` more
than 16 blocks from the entity's own position is refused with an error and `pos` left
as it was, BlockAttachedEntity.txt lines 290–316), and `color` (the dye's name;
default white). The item's `minecraft:cushion/color` component sets the colour when it
is made (`applyImplicitComponent`, lines 565–594), and `get(CUSHION_COLOR)` reports it.
`repositionEntityAfterLoad()` false (BlockAttachedEntity line 320): its loaded position
is kept as saved. Pick result: the cushion item of its colour.

**What the client is sent**: `AddEntity` with the cushion's position (**Inferred**: it
does not override `getAddEntityPacket`, so Entity's with data 0, unlike hanging
entities, section 6), `SetEntityData` with the colour if not white, `SetPassengers`
when someone sits.

### 5.9 `Mannequin` (Mannequin.txt), for the plan's decision only

`extends Avatar` (a LivingEntity shared with players; not read). Type `mannequin`, id
84, 0.6 × 1.8, eye 1.62; tracking range 32, update interval 2, deltas tracked.
Constructor sets `DATA_PLAYER_MODE_CUSTOMISATION` to `ALL_LAYERS` (the OR of every
`PlayerModelPart` mask) (lines 41–59).

Data (Entity's 0–7, LivingEntity's 8–14, Avatar's 15–16 **not read**: SteelMC lists 15
`DATA_PLAYER_MAIN_HAND` `HUMANOID_ARM`, 16 `DATA_PLAYER_MODE_CUSTOMISATION` byte), then
its own, in the static block's order (lines 454–467):

| Id | Accessor | Serializer | Default |
|---|---|---|---|
| 17 | `DATA_PROFILE` | `RESOLVABLE_PROFILE` | `ResolvableProfile$Static.EMPTY` |
| 18 | `DATA_IMMOVABLE` | `BOOLEAN` | false |
| 19 | `DATA_DESCRIPTION` | `OPTIONAL_COMPONENT` | `Optional.of(translatable("entity.minecraft.mannequin.label"))` |

Rules: `isImmobile()` and `!isEffectiveAi()` when immovable (lines 186–210);
description shown unless `hide_description` (lines 152–184). Saved (lines 212–268):
`profile`, `hidden_layers` (a list of model parts, the complement of the mask),
`main_hand`, `pose` (one of standing, crouching, swimming, fall_flying, sleeping),
`immovable`, `description` (only when not the default) or `hide_description: true`.
It has no `interact` or `hurtServer` of its own: LivingEntity's. Loot table
`entities/mannequin` is empty. No item, no spawn egg.

---------------------------------------------------------------------------------------

## 6. Hanging entities: item frames, glow item frames and paintings

Classes: `BlockAttachedEntity` (also the base of `LeashFenceKnotEntity` and `Cushion`,
sections 7.6 and 5.8), `HangingEntity` (direction; `ItemFrame`, `GlowItemFrame`,
`Painting`), and the items `HangingEntityItem` (paintings and frames) and
`ItemFrameItem` (frames only). Line ranges are of the `javap` output of each class.

### 6.0 Findings of this section

1. **The direction is synched data in 26.3**, not a field: `HangingEntity.DATA_DIRECTION`
   (index 8, serializer `DIRECTION`, default `SOUTH`; HangingEntity.txt lines 3–5,
   26–33, 381–389). Setting it (`setDirection`) moves the entity: its position is
   always the centre of a box worked out from `pos` and the direction (6.2). Both are
   also in the add-entity packet (6.8).
2. **A hanging entity never calls `Entity.tick` or `baseTick`.** `BlockAttachedEntity.
   tick` (BlockAttachedEntity.txt lines 34–73) does only `checkBelowWorld` and its
   survival check: no gravity, no `move`, no fluids, no fire, no portal. Its position
   changes only by `setPos`/`setDirection`/a variant or item change.
3. **Survival is checked once every 101 ticks, not every 100**: the counter is
   post-incremented and compared with `>= 100` before the increment's value is used
   (`ticksSinceLastCheck++ >= 100`), so the first check is in the entity's 101st tick
   and then every 101 ticks. The counter is not saved: it starts at 0 at every load.
   A wall broken behind a painting therefore leaves it hanging for up to 101 ticks
   (about five seconds); a player sees this and it is the game's behaviour.
4. **Any non-zero push or move breaks it and drops its item** (`move`, `push`,
   lines 215–278), with no breaker (`dropItem(level, null)`). A piston that moves an
   entity (`move(MoverType.PISTON, …)`, N4) therefore pops a painting or an unfixed
   item frame. A fixed frame ignores both.
5. **Paintings and item frames check survival differently.** A painting needs every
   block behind its whole box to be solid or a diode, no block collision in its box,
   and no other *painting* overlapping it nor any hanging entity of the same direction
   overlapping it (`canCoexist(false)`). An item frame needs only the one block behind
   `pos` to be solid (or a diode, for a frame on a wall), no block collision in its
   0.75 box, and no hanging entity of any kind *with the same direction* overlapping
   that box (`canCoexist(true)`). A fixed frame always survives.
6. **The painting variant is drawn from the new painting's own `random`** (the
   `Entity.random` made by `RandomSource.create()` in `Entity.<init>`, Entity.txt lines
   285–300), *after* its UUID's two `nextLong`, as one `nextInt(n)` over the
   placeable variants of the largest area that fit (6.4). It is drawn **even when the
   item carries a `minecraft:painting_variant` component**: that component is applied
   afterwards and replaces it, and then survival is checked again with it. If no
   placeable variant fits, nothing is placed even for an item with a fixed variant.
7. **Breaking an item frame with an item in it takes two hits**: the first only
   drops (or, for a creative player, deletes) the item and plays the remove-item
   sound; the second breaks the frame (6.5). The frame keeps its `rotation` when its
   item is taken out, so the next item put in starts at that rotation.
8. **In a frame's drop the frame comes first, then its item** (ItemFrame.txt lines
   470–537), and the item's drop draws one `nextFloat()` from the frame's random
   against `dropChance` (1.0 unless set in NBT), so the draw is made though it always
   passes.
9. **A painting's entity chunk can differ from its block's chunk.** Its position is
   its box's centre, which for an even width lies on a block boundary half a block to
   one side of `pos` (6.4): a 2-wide painting facing south at `pos.x = 16k + 15` has
   `x = 16(k+1)` and belongs to chunk `k+1`, while `block_pos` is in chunk `k`. The
   game files an entity by its position (`blockPosition()`), so Clustine must too.
10. **Glow item frames differ only in their sounds and their item** (GlowItemFrame.txt):
    the glow is drawn by the client (**Client**).
11. The **comparator reads an item frame** through a conductor: the repo's
    `crates/clustine-blocks/src/diode.rs` `input_signal` says "an item frame beyond the
    conductor waits for entities" (lines 85–89). N5 must fill it (6.6).

### 6.1 `BlockAttachedEntity` (BlockAttachedEntity.txt)

Fields: `pos` (`BlockPos`, the block the entity is in, not the block it hangs on),
`ticksSinceLastCheck` (int, not saved), `CHECK_INTERVAL = 100`.

- **`tick()`** (lines 34–73), server only:
  ```
  checkBelowWorld()                       // y < minY - 64: onBelowWorld → discard
  if (ticksSinceLastCheck++ >= 100):      // old value compared: first check at the 101st tick
      ticksSinceLastCheck = 0
      tickAtCheckInterval()               // empty here and in the knot; the cushion's is 5.8's
      if !isRemoved() && !survives():
          discard()                       // RemovalReason.DISCARDED
          dropItem(level, null)
  ```
  `ServerLevel.tickNonPassenger` still calls `commonTick()` first (n4 brief 1.1), so
  `tickCount` counts.
- **`setPos(x, y, z)`** (lines 325–338): `pos = BlockPos.containing(x, y, z)`;
  `recalculateBoundingBox()` (which sets the real position, 6.2); `needsSync = true`.
  So the entity's position after `setPos` is not `(x, y, z)` but the centre of its box
  at the block containing that point. `repositionEntityAfterLoad()` is false (lines
  320–323); `refreshDimensions()` and `thunderHit` do nothing (lines 346–352).
- **Saved**: `block_pos` (`BlockPos.CODEC`, an int array `[x, y, z]`; lines 280–288).
  Read (lines 290–316): if absent, or not `closerThan(blockPosition(), 16.0)` (the
  position read from `Pos`), the error "Block-attached entity at invalid position" is
  logged and `pos` is left as `setPos` made it from `Pos`; else `pos = block_pos`.
  Load order: `Entity.load` reads `Pos` and calls `setPos` (so `pos` is first the block
  of `Pos`, the box made with the default direction), then `readAdditionalSaveData`.
- **`isPickable()`** is true (lines 81–84): a player can aim at it.
- **`skipAttackInteraction(attacker)`** (lines 86–112): for a player attacker: if
  `!level.mayInteract(player, pos)` return true (the attack is swallowed); else return
  `hurtOrSimulate(damageSources().playerAttack(player), 0.0f)`. For anything else
  false. **Inferred**: `Player.attack` asks this first and goes no further when it
  returns true, so the player's attack on a hanging entity is exactly this call with
  zero damage (Not read: `Player.attack`).
- **`hurtServer(level, source, amount)`** (lines 154–191):
  ```
  if isInvulnerableToBase(source): return false     // Entity.txt 8588–8640
  if !gamerule mobGriefing && source.getEntity() instanceof Mob: return false
  if !isRemoved():
      kill(level, source.getEntity())               // onKilled(); remove(KILLED); gameEvent(ENTITY_DIE, attacker or this)
      markHurt()
      dropItem(level, source.getEntity())
  return true
  ```
  The amount is ignored: one hit of anything breaks it. `isInvulnerableToBase`: removed;
  or invulnerable (NBT `Invulnerable`) unless the source bypasses invulnerability or is a
  creative player; or fire against a fire-immune entity; or fall against
  `#fall_damage_immune`. `hurtClient` (lines 143–152) is `!isInvulnerableToBase`.
- **`move(type, vec)`** (lines 215–241): on the server, if not removed and
  `vec.lengthSqr() > 0`: `kill(level)` (= `kill(level, this)`, so the game event's
  source is the entity itself) then `dropItem(level, null)`. Nothing moves.
- **`push(dx, dy, dz)`** (lines 243–278): the same, if `dx*dx + dy*dy + dz*dz > 0`
  (exact `double` arithmetic in that order).
- **`ignoreExplosion(explosion)`** (lines 193–213): N6; true if the explosion's direct
  source is in water, else `Entity.ignoreExplosion` only if the explosion affects
  block-like entities, else true.

### 6.2 `HangingEntity` (HangingEntity.txt)

- **Data**: `DATA_DIRECTION` (`EntityDataSerializers.DIRECTION`, default `SOUTH`;
  lines 3–5, 26–33, 381–389). `onSyncedDataUpdated(DATA_DIRECTION)` calls
  `setDirection(getDirection())` (lines 35–48): on the server this runs as well, from
  `SynchedEntityData.set` (**Inferred**, the game's `set` calls
  `entity.onSyncedDataUpdated` on both sides), so `setDirection` runs twice; harmless.
- **`setDirection(d)`** (lines 68–93), the painting's (item frames override, 6.5):
  `Validate.isTrue(d.getAxis().isHorizontal())`; `setDirectionRaw(d)` (sets the data);
  `setYRot((float)(d.get2DDataValue() * 90))` (south 0, west 90, north 180, east 270);
  `yRotO = getYRot()`; `recalculateBoundingBox()`. `xRot` stays 0.
- **`recalculateBoundingBox()`** (lines 95–122): if the direction is null return (the
  data is never null after construction); `box = calculateBoundingBox(pos,
  direction)`; `c = box.getCenter()`; `setPosRaw(c.x, c.y, c.z)`; `setBoundingBox(box)`.
  The position is the box's centre (`AABB.getCenter` = `Mth.lerp(0.5, min, max)` per
  axis, which the repo's `physics/aabb.rs` `lerp` matches).
- **`survives()`** (lines 126–153), for paintings (item frames override, 6.5):
  ```
  if hasLevelCollision(getPopBox()): return false        // popBox = boundingBox here
  ok = level.findBlocksIn(calculateSupportBox()).filterState(isSupportingBlock).allMatched()
  return ok && canCoexist(false)
  ```
  - `hasLevelCollision(box)` (lines 202–220): `!level.noBlockCollision(this, box) ||
    !level.noBorderCollision(this, box)`: a block's collision shape intersecting the box,
    or the world border.
  - `isSupportingBlock(state)` (lines 155–166): `state.isSolid() ||
    DiodeBlock.isDiode(state)` (a repeater or a comparator). `isSolid` is the game's
    deprecated legacy flag the repo has as `props().is_solid()`.
  - `calculateSupportBox()` (lines 168–180): `getBoundingBox().move(direction.step()
    * -0.5f).deflate(1.0E-7)`: the box moved half a block into the wall (the step is a
    `Vector3f`; `±1 * -0.5f` is exactly `∓0.5`), shrunk by `1e-7` on every side, so
    that it covers exactly the blocks behind the painting. **Inferred** (Not read:
    `BlockMatcher`): `findBlocksIn` visits every block from `floor(min)` to `floor(max)`
    per axis and `allMatched` is true when every one passes.
  - `canCoexist(sameDirectionOnly)` (lines 182–200, predicate lines 347–379):
    `!level.hasEntities(EntityTypeTest.forClass(HangingEntity.class), getPopBox(),
    other -> other != this && ((!sameDirectionOnly && other.getType() == getType()) ||
    other.getDirection() == getDirection()))`. For a painting (`false`): any other
    painting overlapping it in any direction, or any hanging entity (an item frame) of
    the same direction overlapping it. The test is box intersection (strict, as
    `AABB.intersects`; **Inferred**, Not read: `Level.hasEntities`), so paintings that
    only touch edge to edge coexist. Leash knots and cushions are `BlockAttachedEntity`,
    not `HangingEntity`, and never block a painting.
- **`getPopBox()`** (lines 222–226): `getBoundingBox()`.
- **`spawnAtLocation(level, stack, yOffset)`** (lines 230–272), the drop of every
  hanging entity:
  ```
  e = new ItemEntity(level,
        getX() + (double)((float)direction.getStepX() * 0.15f),
        getY() + (double)yOffset,
        getZ() + (double)((float)direction.getStepZ() * 0.15f),
        stack)                                  // the entity's own draws: n4 brief 8.4
  e.setDefaultPickUpDelay()                     // 10 ticks
  level.addFreshEntity(e)
  ```
  The 2-argument `spawnAtLocation(level, stack)` passes `yOffset = 0.0f` (Entity.txt
  lines 6374–6381). `0.15f` is widened after the float multiply: `±0.15000000596046448`.
  The item lands 0.15 out from the wall, in front of the entity's centre; for a frame
  on a floor or a ceiling the steps are 0 and it is at the centre.
- `rotate`/`mirror` (lines 274–345): structure placement only; not needed.

### 6.3 Placing: `HangingEntityItem.useOn` (HangingEntityItem.txt lines 17–124)

Reached by `UseItemOn` on a block with a painting, an item frame or a glow item frame
(each item's `HangingEntityItem(type, …)`).

```
pos  = clickedPos.relative(clickedFace)          // the block in front of the face
face = clickedFace
if player != null && !mayPlace(player, face, stack, pos): return FAIL
if type == PAINTING:
    opt = Painting.create(level, pos, face)      // 6.4; draws the variant
    if opt.isEmpty(): return CONSUME             // nothing fits: nothing placed, nothing used
    e = opt.get()
else if type == ITEM_FRAME:      e = new ItemFrame(level, pos, face)
else if type == GLOW_ITEM_FRAME: e = new GlowItemFrame(level, pos, face)
else: return SUCCESS
EntityType.createDefaultStackConfig(level, stack, player).apply(e)
                                                 // the item's custom name, entity data and
                                                 // implicit components (a painting variant)
if e.survives():
    (server) e.playPlacementSound(); level.gameEvent(player, ENTITY_PLACE, e.position());
             level.addFreshEntity(e)
    stack.shrink(1)
    return SUCCESS
return CONSUME
```

- **`mayPlace` of a painting** (lines 126–141): `!face.getAxis().isVertical() &&
  player.mayUseItemAt(pos, face, stack)`: only the four side faces.
- **`mayPlace` of a frame** (`ItemFrameItem`, ItemFrameItem.txt lines 11–28):
  `level.isInsideBuildHeight(pos) && player.mayUseItemAt(pos, face, stack)`: all six
  faces (on a floor a frame faces `UP`, on a ceiling `DOWN`).
- `mayUseItemAt` is the player's build check (`Player.mayUseItemAt`; adventure mode's
  `can_place_on`; Not read). Note the painting's `mayPlace` does not check build height;
  survival does in effect (no block below the world to hang on).
- **Creative**: `shrink(1)` is unconditional here. **Inferred** (Not read:
  `ServerPlayerGameMode.useItemOn`): the server keeps a creative player's count by
  saving and restoring it around `useOn` (as for blocks), so the stack is not used up.
  The repo's creative placement should already do so for blocks.
- **Inferred** (Not read: `EntityType.createDefaultStackConfig`): it applies
  `minecraft:custom_name` (setting the entity's name), `minecraft:entity_data` (NBT
  merged into the entity) and the entity's implicit components; for a painting
  `Painting.applyImplicitComponents` takes `minecraft:painting_variant`
  (Painting.txt lines 76–105) and calls `setVariant`, which remakes its box
  (`onSyncedDataUpdated`, lines 30–41).
- Sound: `playPlacementSound()` → `playSound(ITEM_FRAME_PLACE /
  GLOW_ITEM_FRAME_PLACE / PAINTING_PLACE, 1.0f, 1.0f)` at the entity's position, source
  `NEUTRAL` (`Entity.getSoundSource`, Entity.txt lines 10506–10509), to everyone near,
  the placer included (`level.playSound(null, …)`), unless the entity is silent.

### 6.4 Paintings (Painting.txt, PaintingVariant.txt, PaintingVariants.txt)

**`Painting.create(level, pos, face)`** (lines 107–182):
```
p = new Painting(level, pos)                       // PAINTING type; Entity.<init>: uuid (2 nextLong from p.random)
list = new ArrayList(registry(PAINTING_VARIANT).getTagOrEmpty(#minecraft:placeable))  // in the tag's order
if list.isEmpty(): return empty
p.setDirection(face)
list.removeIf(v -> { p.setVariant(v); return !p.survives(); })   // lambda lines 447–458; in list order
if list.isEmpty(): return empty
max = list.stream().mapToInt(v -> v.value().area()).max().orElse(0)   // area = width * height
list.removeIf(v -> area(v) < max)                  // lambda lines 436–445
v = Util.getRandomSafe(list, p.random)             // Inferred: list.get(p.random.nextInt(list.size()))
if v.isEmpty(): return empty                       // cannot happen: the list is not empty
p.setVariant(v.get()); p.setDirection(face)
return Optional.of(p)
```
- Each `survives()` in the filter is a full painting survival (6.2) with that
  variant's box: block collisions, the blocks behind, and other hanging entities.
- The draw is **one `nextInt(n)` from the new painting's own random** (after its
  UUID's two `nextLong`), even when `n` is 1 (**Inferred** from `Util.getRandomSafe`,
  Not read), and even when the item's component will replace the result.
- The order of the list is the tag file's order
  (`generated/data/minecraft/tags/painting_variant/placeable.json`; the repo's
  `tags.rs` keeps it: ids `24, 1, 0, 2, 5, 32, 47, 35, 12, 37, 42, 13, 46, 22, 26, 8,
  40, 45, 39, 50, 19, 33, 31, 7, 38, 15, 4, 23, 27, 36, 44, 3, 6, 9, 10, 11, 17, 18, 20,
  25, 28, 29, 30, 34, 41, 43, 14`). The draw picks by index in that order after both
  filters, so the order matters for which variant a value gives.

**The box** (`calculateBoundingBox(pos, d)`, lines 256–322; `offsetForPaintingSize`,
lines 324–333):
```
c = Vec3.atCenterOf(pos).relative(d, -0.46875)      // centre of pos, 0.46875 back towards the wall
w = variant.width; h = variant.height              // in blocks
ow = (w % 2 == 0) ? 0.5 : 0.0;  oh = (h % 2 == 0) ? 0.5 : 0.0
c = c.relative(d.getCounterClockWise(), ow).relative(UP, oh)
sx = (d.axis == X) ? 0.0625 : (double)w
sy = (double)h
sz = (d.axis == Z) ? 0.0625 : (double)w
box = AABB.ofSize(c, sx, sy, sz)                   // c ± s/2 on each axis (Not read; Inferred)
```
`Vec3.relative(dir, k)` adds `step * k` per axis (**Inferred**, the step an int widened
to double). `0.46875` and `0.0625` are exact. `getCounterClockWise` (about Y): north →
west, west → south, south → east, east → north. So the painting extends from `pos` to
the counter-clockwise side and upwards for even sizes, centred on `pos` for odd sizes:
a 2×1 facing south at `(x, y, z)` covers `x..x+2`, `y..y+1`, `z..z+0.0625`; a 4×3
covers 2 blocks to each side of the boundary between `pos` and its counter-clockwise
neighbour, and `y-1..y+2`. Its position is the centre: for a 1×1 facing south,
`(x + 0.5, y + 0.5, z + 0.03125)`.

**Data**: `DATA_PAINTING_VARIANT_ID` (index 9, `EntityDataSerializers.PAINTING_VARIANT`,
a `Holder<PaintingVariant>`; lines 3, 15–28, 460–466). Default: `VariantUtils.getAny(
registryAccess, PAINTING_VARIANT)` (**Inferred**: the registry's first entry,
`minecraft:alban`, id 0; Not read: `VariantUtils`). A change remakes the box
(`onSyncedDataUpdated`, lines 30–41). `get(PAINTING_VARIANT)` answers the variant (pick
block, lines 61–74).

**Other**:
- Constructors: `Painting(level, pos)` (private, lines 192–199, direction left at the
  default `SOUTH` until set) and `Painting(level, pos, d, variant)` (lines 201–213:
  `setVariant` then `setDirection`).
- `snapTo(x, y, z, yRot, xRot)` (lines 387–394): `setPos(x, y, z)` only (rotations
  ignored; the direction decides them).
- `trackingPosition()` (lines 396–401): `Vec3.atLowerCornerOf(pos)`, the block's
  corner, not the centre. `ServerEntity` uses it as the base of the position codec
  and for its moves (ServerEntity.txt lines 82, 205).
- `dropItem(level, breaker)` (lines 335–376): if `!gamerule entityDrops` return (no
  sound either); `playSound(PAINTING_BREAK, 1, 1)`; if the breaker is a player with
  infinite materials (creative) return; else `spawnAtLocation(level, ItemStack(
  PAINTING) with CUSTOM_NAME = getCustomName())`. **The dropped painting carries no
  variant**: a painting placed from it draws again.
- `getPickResult()` (lines 428–434): a plain painting item.
- `interact`: not overridden: `Entity.interact` (Entity.txt lines 6475–…; a lead's
  path for `Leashable`, which a painting is not) gives `PASS`.
- **Saved** (lines 215–254): `facing` (`Direction.LEGACY_ID_CODEC_2D`, a byte of the 2D
  value: S 0, W 1, N 2, E 3) written *before* the super's `block_pos`; the variant by
  `VariantUtils.writeVariant` (**Inferred**: key `variant`, the registry key as a
  string). Read: `facing` (default `SOUTH`), super, `setDirection`, then the variant if
  present.

**Variants** (`generated/data/minecraft/painting_variant/*.json`, 51; the network id is
the index in the repo's `registries.rs` order, which is alphabetical; "tag" is the
index in `#minecraft:placeable`, "no" for the four not placeable):

| id | name | w×h | area | tag | | id | name | w×h | area | tag |
|---|---|---|---|---|---|---|---|---|---|---|
| 0 | alban | 1×1 | 1 | 2 | | 26 | match | 2×2 | 4 | 14 |
| 1 | aztec | 1×1 | 1 | 1 | | 27 | meditative | 1×1 | 1 | 28 |
| 2 | aztec2 | 1×1 | 1 | 3 | | 28 | orb | 4×4 | 16 | 40 |
| 3 | backyard | 3×4 | 12 | 31 | | 29 | owlemons | 3×3 | 9 | 41 |
| 4 | baroque | 2×2 | 4 | 26 | | 30 | passage | 4×2 | 8 | 42 |
| 5 | bomb | 1×1 | 1 | 4 | | 31 | pigscene | 4×4 | 16 | 22 |
| 6 | bouquet | 3×3 | 9 | 32 | | 32 | plant | 1×1 | 1 | 5 |
| 7 | burning_skull | 4×4 | 16 | 23 | | 33 | pointer | 4×4 | 16 | 21 |
| 8 | bust | 2×2 | 4 | 15 | | 34 | pond | 3×4 | 12 | 43 |
| 9 | cavebird | 3×3 | 9 | 33 | | 35 | pool | 2×1 | 2 | 7 |
| 10 | changing | 4×2 | 8 | 34 | | 36 | prairie_ride | 1×2 | 2 | 29 |
| 11 | cotan | 3×3 | 9 | 35 | | 37 | sea | 2×1 | 2 | 9 |
| 12 | courbet | 2×1 | 2 | 8 | | 38 | skeleton | 4×3 | 12 | 24 |
| 13 | creebet | 2×1 | 2 | 11 | | 39 | skull_and_roses | 2×2 | 4 | 18 |
| 14 | dennis | 3×3 | 9 | 46 | | 40 | stage | 2×2 | 4 | 16 |
| 15 | donkey_kong | 4×3 | 12 | 25 | | 41 | sunflowers | 3×3 | 9 | 44 |
| 16 | earth | 2×2 | 4 | no | | 42 | sunset | 2×1 | 2 | 10 |
| 17 | endboss | 3×3 | 9 | 36 | | 43 | tides | 3×3 | 9 | 45 |
| 18 | fern | 3×3 | 9 | 37 | | 44 | unpacked | 4×4 | 16 | 30 |
| 19 | fighters | 4×2 | 8 | 20 | | 45 | void | 2×2 | 4 | 17 |
| 20 | finding | 4×2 | 8 | 38 | | 46 | wanderer | 1×2 | 2 | 12 |
| 21 | fire | 2×2 | 4 | no | | 47 | wasteland | 1×1 | 1 | 6 |
| 22 | graham | 1×2 | 2 | 13 | | 48 | water | 2×2 | 4 | no |
| 23 | humble | 2×2 | 4 | 27 | | 49 | wind | 2×2 | 4 | no |
| 24 | kebab | 1×1 | 1 | 0 | | 50 | wither | 2×2 | 4 | 19 |
| 25 | lowmist | 4×2 | 8 | 39 | | | | | | |

`PaintingVariant` is a record `(width, height, assetId, title, author)` (lines 21–40;
`area()` = `width * height`, lines 42–50); the codec bounds width and height by
`ExtraCodecs.intRange` (lines 100–135) and the network codec is `VAR_INT, VAR_INT,
Identifier, optional component, optional component` (lines 138–154), used for an
inline holder. `PaintingVariants` (the bootstrap) registers the same sizes; `earth`,
`wind`, `water`, `fire` and `wither` without an author (the `IIZ` overload).

### 6.5 Item frames (ItemFrame.txt) and glow item frames (GlowItemFrame.txt)

**Fields and data**: `dropChance` (float, default 1.0), `fixed` (bool, default false),
invisible (the shared flag, `setInvisible(false)` in the constructors). Data:
`DATA_ITEM` (index 9, `ITEM_STACK`, default `EMPTY`), `DATA_ROTATION` (index 10, `INT`,
default 0) (lines 3–25, 75–91, 1036–1046). Constants `DEPTH 0.0625f`, `WIDTH = HEIGHT =
0.75f`, `NUM_ROTATIONS 8`.

**`setDirection(d)`** (lines 93–137), any of six:
```
setDirectionRaw(d)
if d horizontal: xRot = 0; yRot = (float)(d.get2DDataValue() * 90)
else:            xRot = (float)(-90 * d.getAxisDirection().getStep()); yRot = 0     // UP -90, DOWN 90
xRotO = xRot; yRotO = yRot
recalculateBoundingBox()            // final: super's, then syncPacketPositionCodec(x, y, z) (lines 139–151)
```

**The box** (`createBoundingBox(pos, d, hasMap)`, lines 174–228;
`calculateBoundingBox` passes `hasFramedMap()`, lines 153–161; `getPopBox` passes
`false`, lines 163–172):
```
c  = Vec3.atCenterOf(pos).relative(d, -0.46875)
fw = hasMap ? 1.0f : 0.75f;  fh = hasMap ? 1.0f : 0.75f
sx = (d.axis == X) ? 0.0625 : (double)fw
sy = (d.axis == Y) ? 0.0625 : (double)fh
sz = (d.axis == Z) ? 0.0625 : (double)fw
return AABB.ofSize(c, sx, sy, sz)
```
A frame with a map is a whole block wide; its survival box (the pop box) is always
0.75. Changing the item remakes the box (`onItemChanged`, lines 669–673, from `setItem`
and from `onSyncedDataUpdated(DATA_ITEM)`, lines 654–667). Example: a frame on the
north face of the block at `(0, 64, 0)`: `pos = (0, 64, -1)`, facing north; box `x
0.125..0.875`, `y 64.125..64.875`, `z -0.0625..0.0`; position `(0.5, 64.5, -0.03125)`.

**`survives()`** (lines 230–270):
```
if fixed: return true
if hasLevelCollision(getPopBox()): return false
behind = level.getBlockState(pos.relative(direction.getOpposite()))
if !behind.isSolid() && !(direction horizontal && DiodeBlock.isDiode(behind)): return false
return canCoexist(true)            // no other hanging entity of the same direction in the pop box
```

**`move` and `push`** (lines 272–293): do nothing when fixed; else the super's (break).

**Damage** (lines 303–403):
- `canHurtWhenFixed(source)`: the source bypasses invulnerability or is a creative
  player. `shouldDamageDropItem(source)`: not `#is_explosion` and the frame holds an
  item.
- `hurtServer(level, source, amount)`:
  ```
  if fixed: return canHurtWhenFixed(source) && super.hurtServer(...)   // breaks; drops nothing (fixed)
  if isInvulnerableToBase(source): return false
  if shouldDamageDropItem(source):
      dropItem(level, source.getEntity(), false)      // only the item (below)
      gameEvent(BLOCK_CHANGE, source.getEntity())
      playSound(getRemoveItemSound(), 1, 1)           // ITEM_FRAME_REMOVE_ITEM / GLOW_…
      return true
  return super.hurtServer(...)                         // BlockAttachedEntity: kill, markHurt, dropItem(level, attacker)
  ```
  So a survival or creative player's first hit takes the item out and the second
  breaks the frame. The mob-griefing rule applies only on the second (the super's).
- `hurtClient` (lines 332–349): false when fixed unless `canHurtWhenFixed`; else
  `!isInvulnerableToBase`.
- `onKilled()` (lines 295–301): `removeFramedMap(getItem())` (maps: later).

**Drops** (public `dropItem(level, breaker)`, lines 431–448; private `dropItem(level,
breaker, dropSelf)`, lines 470–537):
```
dropItem(level, breaker):                   // the break: survival lost, a push, or a hit that kills
    playSound(getBreakSound(), 1, 1)        // ITEM_FRAME_BREAK / GLOW_ITEM_FRAME_BREAK, always
    dropItem(level, breaker, true)
    gameEvent(BLOCK_CHANGE, breaker)

dropItem(level, breaker, dropSelf):
    if fixed: return
    item = getItem(); setItem(EMPTY)        // setItem: data set, box remade, comparators told (below)
    if !gamerule entityDrops: if breaker == null: removeFramedMap(item); return
    if breaker is a player with infinite materials: removeFramedMap(item); return   // creative: nothing drops
    if dropSelf: spawnAtLocation(level, getFrameItemStackWithData())   // the frame first, named as the entity
    if !item.isEmpty():
        item = item.copy(); removeFramedMap(item)
        if random.nextFloat() < dropChance: spawnAtLocation(level, item)
```
`getFrameItemStackWithData()` (lines 980–1000): `ItemStack(ITEM_FRAME)` (glow:
`GLOW_ITEM_FRAME`, GlowItemFrame.txt lines 46–51) with `CUSTOM_NAME = getCustomName()`.

**Items** (lines 562–632):
- `setItem(stack)` = `setItem(stack, true)`: if not empty, `stack = stack.copyWithCount(1)`;
  `onItemChanged(stack)` (box); `entityData.set(DATA_ITEM, stack)`; if not empty
  `playSound(getAddItemSound(), 1, 1)` (`ITEM_FRAME_ADD_ITEM` / glow); if the flag and
  `pos != null`: `level.updateNeighbourForOutputSignal(pos, Blocks.AIR)` (comparators,
  6.6). Loading calls `setItem(stack, false)`: no update, but **the add-item sound plays
  on load** of a frame with an item if the entity is in a level then (**Inferred**: the
  sound goes nowhere before the entity is added).
- `setRotation(r)` = `setRotation(r, true)` (lines 685–714): `DATA_ROTATION = r % 8`;
  with the flag and `pos != null`, `updateNeighbourForOutputSignal(pos, AIR)`.
- `getSlot(0)` is the item (lines 639–652), for commands.

**Interaction** (`interact(player, hand, location)`, lines 827–917):
```
held = player.getItemInHand(hand)
hasItem = !getItem().isEmpty(); holding = !held.isEmpty()
if fixed: return PASS
(client: SUCCESS if hasItem || holding, else PASS)
if !hasItem:
    if holding && !isRemoved():
        map = MapItem.getSavedData(held, level)
        if map != null && map.isTrackedCountOverLimit(256): return FAIL      // maps: later
        setItem(held)                                    // one of it, the add sound, comparators
        gameEvent(BLOCK_CHANGE, player)
        held.consume(1, player)                          // Inferred: not used up by a creative player
        return SUCCESS
    return PASS
playSound(getRotateItemSound(), 1, 1)                    // ITEM_FRAME_ROTATE_ITEM / glow
setRotation(getRotation() + 1)                           // wraps at 8; comparators told
gameEvent(BLOCK_CHANGE, player)
return SUCCESS
```
Whatever the player holds, a frame with an item rotates it; the held item is not
used. An invisible frame behaves the same (invisibility is only drawn).

**`getAnalogOutput()`** (lines 924–938): `0` if empty, else `getRotation() % 8 + 1`
(1 to 8).

**Saved** (lines 716–825): `Item` (`ItemStack.CODEC`, only if not empty), `ItemRotation`
(byte), `ItemDropChance` (float), `Facing` (`Direction.LEGACY_ID_CODEC`, a byte of the
3D value: D 0, U 1, N 2, S 3, W 4, E 5), `Invisible` (bool), `Fixed` (bool), after the
super's `block_pos`. Read: the item (default `EMPTY`; a map replaced is removed from its
data), `setItem(item, false)`; `setRotation(ItemRotation or 0, false)`;
`dropChance = ItemDropChance or 1.0f`; `setDirection(Facing or DOWN)`;
`setInvisible(Invisible or false)`; `fixed = Fixed or false`. Fixed and invisible are
set only by NBT (commands, `minecraft:entity_data` on the item); a creative player can
place a frame whose item carries them.

**Other**: `getPickResult()` (lines 965–978): a copy of its item, else the frame
item. `getVisualRotationYInDegrees` (lines 1002–1034) and `shouldRenderAtSqrDistance`
are the client's. The glow item frame overrides only the five sounds and its item
(GlowItemFrame.txt lines 21–51).

### 6.6 Comparators and item frames (for the repo)

- **What tells comparators**: `setItem(…, true)` and `setRotation(…, true)` call
  `level.updateNeighbourForOutputSignal(pos, Blocks.AIR)` at the **frame's own block**
  (the air in front of the wall). **Inferred** (Not read:
  `Level.updateNeighbourForOutputSignal`): it tells a comparator in each horizontal
  neighbour of `pos`, and through a conductor neighbour the comparator beyond it, as the
  repo does for block entities (`crates/clustine-sim/src/region/updates.rs` around
  lines 243 and 459). The removal of the item when the frame breaks goes through
  `setItem(EMPTY)` (6.5), so it tells them too; `kill`/`discard` alone does not.
- **What a comparator reads**: (Not read: `ComparatorBlock.getInputSignal`,
  `getItemFrame`; **Inferred**, consistent with SteelMC's port): behind a conductor that
  is not powered whole, at the block beyond it, the item frames whose box intersects
  that block's unit box and whose direction equals the comparator's facing; if exactly
  one, its `getAnalogOutput()`; the reading is the larger of that and the block's analog
  output where both exist. A frame on the far side of the wall, facing away from the
  comparator, is read.
- The repo's `clustine_blocks::diode::input_signal` (lines 85–107) takes only a
  `BlockView`; N5 needs a view of entities (frames by block and direction) in it, from
  the region and its copies.

### 6.7 What the region keeps of a hanging entity

Beside the common fields: `pos` (the block), `direction`, and per type: a painting's
variant id; a frame's item (`ItemStack`, count 1, with components), rotation (0–7),
`drop_chance` (f32), `fixed`, `invisible`; and `ticks_since_check` (0–100). The game
does not save the counter (a load starts it at 0), so **Inferred**: Clustine may treat
a take-in (load, merge, split) as the game's load and start it at 0, or keep it in the
record; either is the game's behaviour at some load. Position, box and rotations are
derived from `pos` and the direction (and the variant or the map) and need no storing.
The repo's `EntityData` (`crates/clustine-world/src/entity.rs` lines 81–102) has
`Item` and `FallingBlock` only.

### 6.8 What clients are sent

- **Add**: `getAddEntityPacket(serverEntity)` (ItemFrame.txt lines 940–951,
  Painting.txt lines 403–414): `new ClientboundAddEntityPacket(this,
  getDirection().get3DDataValue(), getPos())`. **Inferred** (Not read: that
  constructor): the position written is the block's corner `(pos.x, pos.y, pos.z)` as
  doubles, the rotations the entity's, the data the direction's 3D value (D 0, U 1, N 2,
  S 3, W 4, E 5), the motion zero. The client rebuilds the entity from these
  (`recreateFromPacket`, ItemFrame.txt lines 953–963, Painting.txt lines 416–426:
  `Entity.recreateFromPacket` then `setDirection(Direction.from3DDataValue(data))`; its
  `snapTo` → `setPos` → `pos = BlockPos.containing(corner)` and the box), so the
  centre never needs to be sent.
- **Data at pairing**: the non-default values: index 8 `DIRECTION` (a varint of the 3D
  value) unless `SOUTH`; a painting's index 9 variant (always, unless the default);
  a frame's index 9 item and index 10 rotation if set; index 0's invisible bit
  (`0x20`) for an invisible frame.
- **Afterwards**: entity types `painting`, `item_frame`, `glow_item_frame` have
  `updateInterval = Integer.MAX_VALUE` and `trackDeltas = false` (8 below), so
  `ServerEntity.sendChanges` enters its update branch only on `needsSync` (set by
  `setPos`), a dirty data value (the item put in or taken out, a rotation), or its first
  tick (`tickCount % MAX_VALUE == 0` at 0; **Inferred** for `UpdateInterval`, Not read).
  For an item frame, every tick whose `tickCount % 10 == 0` also sends its map's updates
  to every player and then `sendDirtyEntityData()` (ServerEntity.txt lines 53–201; maps
  later). The base of a painting's position codec is `trackingPosition()`, the block's
  corner (Painting.txt lines 396–401; ServerEntity.txt lines 82, 205), a frame's its
  centre. The repo's `services/edge/src/tracking.rs` `Tracker` plays this with the
  type's interval and must take the corner as a painting's base.
- **Removal**: the usual `RemoveEntities`. The break and place sounds are
  `ClientboundSoundPacket`s from `level.playSound(null, …)` (**Inferred**: each draws
  its seed from the level's random; Not read: `ServerLevel.playSeededSound`).

---------------------------------------------------------------------------------------

## 7. Leashes

Read from `Leashable.txt`, `Leashable$LeashData.txt`, `Leashable$Wrench.txt`,
`LeashFenceKnotEntity.txt`, `BlockAttachedEntity.txt`, `LeadItem.txt`, `Entity.txt`,
`AbstractBoat.txt` and `ServerEntity.txt`. Line ranges are of those files.

### 7.0 Findings of this section

1. **Boats are the only leashable entities of N5.** Of the N5 classes only
   `AbstractBoat` implements `Leashable` (AbstractBoat.txt line 2): every boat, chest
   boat, raft and chest raft. Minecarts (`AbstractMinecart extends VehicleEntity`),
   armour stands (`LivingEntity`), the mannequin (`Avatar`), cushions and the hanging
   entities do not. Every mob that can be leashed is a `Mob` (N7; `Mob` not read).
   So N5's leashes are: a boat held by a player, a boat tied to a fence (a knot), and a
   boat held by **another boat** (any entity can be a holder: section 7.4, the first
   branch of `Entity.interact`). A boat tied to a boat is reachable with no mob at all:
   lead a boat, sneak and use another boat.
2. **A leash has two kinds of pull, and the old "pull towards the holder" is gone.**
   26.3's `tickLeash` has no legacy elastic pull. Past `leashElasticDistance` (6, less
   both widths) it applies a damped spring between attachment points as a *wrench*: a
   force on the motion and a torque on an angular momentum kept in `LeashData` that
   turns the leashee's yaw every tick (section 7.3). Boats support the four-point
   ("quad") leash, but only a holder that `supportQuadLeashAsHolder()` uses it; that is
   `false` for every N5 holder (`Entity`, line 11079–11082; the happy ghast overrides it,
   not read), so N5 uses the single points.
3. **A leash too far (more than 12 between box centres) breaks with a lead dropped**, at
   the leashee, whatever the game mode. A holder no longer in the world (removed,
   unloaded, dead, a spectator) also drops the leash, with the lead if the gamerule
   `entityDrops` holds and without it otherwise. Breaking a knot drops no item from the
   knot, but each leashee drops its own lead on its next tick, **even when a creative
   player broke the knot** (section 7.6).
4. **A saved leash restores by UUID or by fence position, for 100 ticks.** If the holder
   is not found by `tickCount > 100` (counted from the leashee's load), the leashee
   drops a lead (gamerule not checked) and forgets the leash. A fence position restores
   by finding or **creating** a knot there, even with no fence (the knot then goes at its
   first check, 101 ticks later).
5. **The knot holds no list.** A knot saves nothing but its block; its leashees find it by
   position. It discards itself when its last leashee lets go (`notifyLeasheeRemoved`),
   at its check every 101 ticks if its block is not a fence, and when moved or pushed.
6. **`SetEntityLink` (id 102) is not in the repo** beyond its id
   (`crates/clustine-protocol/src/generated/packet_ids.rs:251`); no encoder or use.

### 7.1 Data (`Leashable$LeashData`, LeashData.txt lines 1–96)

Fields: `leashHolder: Entity` (the live reference), `delayedLeashInfo: Either<UUID,
BlockPos>` (from a save, until restored), `delayedLeashHolderId: int` (client only, set
by `SetEntityLink`), `angularMomentum: double` (not saved; 0 on load).
`setLeashHolder(e)` sets the holder and clears the delayed info and id (lines 40–51).

**Saved** (`writeLeashData`, Leashable.txt lines 127–134; `readLeashData` 105–125) under
the key `"leash"`, written with `storeNullable` (absent when there is no leash). The codec
(LeashData.txt lines 83–95) is `xor(UUIDUtil.CODEC.fieldOf("UUID").codec(),
BlockPos.CODEC)`:
- an entity holder: a compound `{UUID: [I; a, b, c, d]}`;
- a knot holder: the knot's block as `BlockPos.CODEC`, an int array `[I; x, y, z]`
  (lines 53–81: a `LeashFenceKnotEntity` holder writes `Either.right(knot.getPos())`,
  any other holder `Either.left(uuid)`, a data not yet restored writes its delayed info
  back).
Reading: if the entity had leash data and the save has none, `removeLeash()` first;
then `setLeashData(read)` (a data with only the delayed info). `AbstractBoat` writes and
reads it in `addAdditionalSaveData`/`readAdditionalSaveData` (AbstractBoat.txt lines
2008–2022).

`isLeashed()` = data non-null and holder non-null (lines 29–41). `mayBeLeashed()` = data
non-null (43–51).

### 7.2 `Leashable.tickLeash(level, e)` (Leashable.txt lines 276–405)

Called at the very end of `Entity.baseTick` on the server for every `Leashable`
(Entity.txt lines 1087–1102), so for a boat inside its call to `Entity.tick`
(AbstractBoat.txt line 429), after `baseTick`'s fire, fluid and portal work. Hanging
entities (a knot) do not call `baseTick` (`BlockAttachedEntity.tick` does not call
`super.tick`), but a knot is never a leashee.

```
d = e.getLeashData()
if d != null && d.delayedLeashInfo != null: restoreLeashFromSave(e, d)        // 7.2.1
if d == null || d.leashHolder == null: return
if !e.canInteractWithLevel() || !d.leashHolder.canInteractWithLevel():
    if gamerule entityDrops: e.dropLeash()        // packet + lead item
    else:                    e.removeLeash()      // packet, no item
h = e.getLeashHolder()                            // server: d.leashHolder (null after a drop)
if h != null && h.level() == e.level():
    dist = e.leashDistanceTo(h)                   // box centre to box centre
    e.whenLeashedTo(h)                            // h.notifyLeashHolder(e): no-op but in Mob/happy ghast
    if dist > e.leashSnapDistance():              // 12.0
        level.playSound(null, h.x, h.y, h.z, LEAD_BREAK, NEUTRAL, 1.0f, 1.0f)
        e.leashTooFarBehaviour()                  // default: dropLeash() (packet + item)
    else if dist > e.leashElasticDistance() - (double)h.getBbWidth() - (double)e.getBbWidth()
            && e.checkElasticInteractions(h, d):  // 7.3; true if a wrench applied
        e.onElasticLeashPull()                    // checkFallDistanceAccumulation
    else:
        e.closeRangeLeashBehaviour(h)             // default no-op
    e.setYRot((float)((double)e.getYRot() - d.angularMomentum))
    d.angularMomentum *= (double)angularFriction(e)
```

- `canInteractWithLevel()` = `isAlive() && !isRemoved() && !isSpectator()` (Entity.txt
  lines 525–539). A player holder who logs out, dies or becomes a spectator, a holder
  unloaded with its chunk, a knot broken: the leash drops on the leashee's next tick.
- The widths are `float` widened (`f2d`) before the subtraction; the subtraction is
  `(6.0 - w_h) - w_e`. For a boat (width 1.375) held by a player (0.6): 4.025; by a knot
  (0.375): 4.25; by a boat: 3.25.
- After a snap or a drop, `d` is still the old object, so the yaw is turned once more by
  its momentum (harmless; port as is).
- `leashSnapDistance()` 12.0, `leashElasticDistance()` 6.0 by default (lines 416–424);
  `AbstractBoat` does not override them. Constants in the interface: `LEASH_TOO_FAR_DIST`
  12.0, `LEASH_ELASTIC_DIST` 6.0, `MAXIMUM_ALLOWED_LEASHED_DIST` 16.0 (not used in the
  interface's code; **Inferred**: by `Mob`), `SPRING_DAMPENING` 0.7f (not used here),
  `TORSIONAL_ELASTICITY` 10.0, `STIFFNESS` 0.11 (lines 1–23).
- `onElasticLeashPull()` (lines 407–414) → `checkFallDistanceAccumulation()` (Entity.txt
  lines 8188–8204): if `motion.y > -0.5 && fallDistance > 1.0`: `fallDistance = 1.0`.
- `angularFriction(e)` (lines 426–447): on ground,
  `level.getBlockState(getBlockPosBelowThatAffectsMyMovement()).getBlock().getFriction()
  * 0.91f` (float); else in a liquid (`isInLiquid`) `0.8f`; else `0.91f`. Widened before
  the multiply.

#### 7.2.1 `restoreLeashFromSave(e, d)` (lines 136–200)

Only on the server, only while `d.delayedLeashInfo != null`, every tick until it ends:
- `Left(uuid)`: `h = serverLevel.getEntity(uuid)`; if found, `setLeashedTo(e, h, true)`
  (sends the link) and return.
- `Right(pos)`: `setLeashedTo(e, LeashFenceKnotEntity.getOrCreateKnot(level, pos),
  true)` and return: always succeeds, creating a knot if none is there (7.5).
- not found: if `e.tickCount > 100`: `e.spawnAtLocation(level, Items.LEAD)` (no gamerule
  check) and `setLeashData(null)` (no packet: nobody was told of the link). Otherwise it
  tries again next tick.

`tickCount` is from the leashee's load (N4 brief 1.1), so a holder must be found within
100 ticks of the leashee's own taking in.

### 7.3 The elastic pull (`checkElasticInteractions`, lines 466–535)

```
quad = h.supportQuadLeashAsHolder() && e.supportQuadLeash()   // false in N5 (finding 2)
ePts = quad ? SHARED_QUAD_ATTACHMENT_POINTS : ENTITY_ATTACHMENT_POINT     // [(0, 0.5, 0.5)]
hPts = quad ? SHARED_QUAD_ATTACHMENT_POINTS : LEASHER_ATTACHMENT_POINT    // [(0, 0.5, 0)]
wrenches = computeElasticInteraction(e, h, ePts, hPts)
if wrenches.isEmpty(): return false
w = Wrench.accumulate(wrenches).scale(quad ? 0.25 : 1.0)
d.angularMomentum += 10.0 * w.torque
rel = getHolderMovement(h).subtract(e.getKnownMovement())
e.addDeltaMovement(w.force.multiply(AXIS_SPECIFIC_ELASTICITY).add(rel.scale(0.11)))
return true
```

`AXIS_SPECIFIC_ELASTICITY` = `(0.8, 0.2, 0.8)`; `SHARED_QUAD_ATTACHMENT_POINTS` =
`(-0.5, 0.5, 0.5), (-0.5, 0.5, -0.5), (0.5, 0.5, -0.5), (0.5, 0.5, 0.5)` (static
initialiser, lines 1009–1060). `addDeltaMovement(v)` = `setDeltaMovement(motion + v)`.

`getHolderMovement(x)` (lines 537–552): `Vec3.ZERO` for a `Mob` with `NoAI`, else
`x.getKnownMovement()`. `Entity.getKnownMovement()` (Entity.txt lines 11467–11486): if
the controlling passenger is a `Player` and the entity is alive, the **player's**
`getKnownMovement()` (`Player` not read), else `getDeltaMovement()`. So a rowed boat's
movement here is its rower's, and a player holder's is the player's known movement.

`computeElasticInteraction(e, h, ePts, hPts)` (lines 554–654):

```
el = e.leashElasticDistance()                        // 6.0
mv = getHolderMovement(e)                            // the LEASHEE's movement (aload_0)
ae = e.getYRot() * 0.017453292f                      // float
se = Vec3((double)e.getBbWidth(), (double)e.getBbHeight(), (double)e.getBbWidth())
ah = h.getYRot() * 0.017453292f
sh = Vec3(h width, h height, h width)
for i in 0 .. ePts.size():                           // one pair for single points
    oe = ePts[i].multiply(se).yRot(-ae)              // fneg of the float product
    pe = e.position().add(oe)
    oh = hPts[i].multiply(sh).yRot(-ah)
    ph = h.position().add(oh)
    computeDampenedSpringInteraction(ph, pe, el, mv, oe).ifPresent(list::add)
```

Note `e.position()` (the feet), not the box centre. `Vec3.yRot(float a)`: **Inferred**
(`Vec3` not read) `c = Mth.cos(a), s = Mth.sin(a)` (the table, the repo's
`clustine_noise::trig`, with the float angle widened); `x' = x*c + z*s`, `y' = y`,
`z' = z*c - x*s`, each float widened to double before its multiply.

`computeDampenedSpringInteraction(ph, pe, el, mv, oe)` (lines 656–703):

```
dist = pe.distanceTo(ph)
if dist < el: return empty
f = ph.subtract(pe).normalize().scale(dist - el)
torque = Wrench.torqueFromForce(oe, f)               // oe.z*f.x - oe.x*f.z
if mv.dot(f) >= 0.0: f = f.scale(0.30000001192092896)    // 0.3f widened
return Wrench(f, torque)
```

The spring is compared with the full 6.0 between attachment points, while `tickLeash`
entered it at `6 - widths` between box centres; between the two it falls to
`closeRangeLeashBehaviour` (no-op).

`Leashable$Wrench` (Wrench.txt lines 1–152): a record `(force: Vec3, torque: double)`;
`ZERO = (Vec3.ZERO, 0.0)`; `torqueFromForce(r, f) = r.z * f.x - r.x * f.z`;
`accumulate(list)`: `ZERO` if empty, else component-wise sums of the forces (x, y, z in
order) and of the torques, summed in list order from `0.0`; `scale(k)` = `(force.scale(k),
torque * k)`.

Offsets that are not physics: `getLeashOffset()` (default `(0, eyeHeight, width *
0.4f)`, lines 805–822; the boat's `(0, (double)(0.88f * height), (double)(0.64f *
width))`, AbstractBoat.txt lines 880–896), `getQuadLeashOffsets()` (the boat's
`createQuadLeashOffsets(this, 0.0, 0.64, 0.382, 0.88)`, lines 903–911; default `(0, 0.5,
0.5, 0.5)`), `Entity.getQuadLeashHolderOffsets()` (`(0, 0.5, 0.5, 0)`, Entity.txt lines
11069–11077), `getRopeHoldPosition` (the knot's: its interpolated position + `(0, 0.2,
0)`, LeashFenceKnotEntity.txt lines 368–377): where the rope is drawn (**Client**). The
server never reads them in N5. `createQuadLeashOffsets(e, a, b, c, d)` (lines 721–797):
with `w = width`, `z0 = a*w`, `zs = b*w`, `xs = c*w`, `y = d*height`: `(-xs, y, zs+z0)`,
`(-xs, y, -zs+z0)`, `(xs, y, -zs+z0)`, `(xs, y, zs+z0)`.

### 7.4 Taking, giving and cutting a leash (`Entity.interact`, Entity.txt lines 6475–6679)

`AbstractBoat.interact` (AbstractBoat.txt lines 2024–2056) calls `VehicleEntity.interact`
first, which is `Entity.interact` (VehicleEntity does not override it); only a `PASS`
from it lets the boat be boarded (not sneaking, `outOfControlTicks < 60f`:
`startRiding`). The packet path is `ServerGamePacketListenerImpl.handleInteract` →
`Player.interactOn` (not read; section 2), which calls `entity.interact` first.
**Inferred** (from the game as known; `Player.interactOn` not read): a player with
infinite materials gets back the count an `interact` took, so a creative player keeps
the lead.

`Entity.interact(player, hand, location)`, in order, with `e = this`:

1. **Tie the player's leashees to this entity** (lines 6477–6555): on the server, if the
   player is sneaking (`isSecondaryUseActive`), `e` is `Leashable`, `canBeLeashed()`
   (always true by default, lines 85–88), alive and not a baby (`LivingEntity.isBaby`):
   `list = leashableInArea(e, l -> l.getLeashHolder() == player)`. For each in list
   order, if `l.canHaveALeashAttachedTo(e)`, `l.setLeashedTo(e, true)`. If any was tied:
   game event `ENTITY_ACTION` at `e.blockPosition()` (context: the player),
   `e.playSound(LEAD_TIED)` (volume 1, pitch 1, unless silent), return
   `SUCCESS_SERVER.withoutItem()`. Whatever the hand holds. This is how a boat becomes a
   holder (finding 1). Otherwise fall through.
2. **Shears** (lines 6556–6574): if the hand holds `Items.SHEARS` and
   `shearOffAllLeashConnections(player)`: `stack.hurtAndBreak(1, player, hand)`, return
   `SUCCESS`. `shearOffAllLeashConnections` (6681–6712) = `dropAllLeashConnections`
   (6714–6763): `list = leashableLeashedTo(e)` (all held by `e`); if `e` is leashed,
   `e.dropLeash()`; each in list `dropLeash()` (each drops a lead, sends the unlink);
   true if anything was dropped, then game event `SHEAR` (the player). If true: on the
   server `level.playSound(null, e.blockPosition(), SHEARS_SNIP, player's sound
   source)`.
3. A `Mob` with shears shearing its equipment (6575–6601): N7.
4. **The holder lets go** (6602–6638): if `e` is alive and `Leashable` and its holder is
   this player: on the server, creative (`hasInfiniteMaterials`) `removeLeash()` (no
   item), else `dropLeash()` (the lead dropped at `e`); game event `ENTITY_INTERACT`;
   `playSound(LEAD_UNTIED)`; return `SUCCESS.withoutItem()` (on both sides).
5. **Leash it to the player** (6639–6677): if the hand holds `Items.LEAD` and the
   current holder is not a `Player` (none, a knot, a boat): client `CONSUME`; server: if
   `canHaveALeashAttachedTo(player)`: if leashed, `dropLeash()` (**the old leash drops a
   lead**: taking a boat off a knot with a lead in hand leaves a lead on the ground),
   `setLeashedTo(player, true)`, `playSound(LEAD_TIED)`, `stack.shrink(1)`, return
   `SUCCESS_SERVER`.
6. `PASS`.

`canHaveALeashAttachedTo(h)` (lines 53–71): `h != this`, `leashDistanceTo(h) <=
leashSnapDistance()` (12, box centres), `canBeLeashed()`.

`leashableInArea(level, centre, pred)` (lines 955–979): `getEntitiesOfClass(Entity.class,
AABB.ofSize(centre, 32, 32, 32), x -> x instanceof Leashable && pred(x))`: a box of
±16 on each axis around the centre; `leashableInArea(e, pred)` uses `e`'s box centre;
`leashableLeashedTo(h)` = those whose `getLeashHolder() == h` around `h`'s box centre
(936–1007). The order is the level's entity lookup order (by section, then insertion).

### 7.5 `setLeashedTo`, `dropLeash`, `removeLeash` and the packet

`setLeashedTo(h, send)` (lines 824–895): nothing if `h == this`. If there is no data:
`data = LeashData(h)`. Otherwise `old = data.leashHolder; data.setLeashHolder(h)` and,
if `old != null && old != h`, `old.notifyLeasheeRemoved(this)` (a knot discards itself if
it now holds nothing, 7.6). If `send` and server: `sendToTrackingPlayers(this,
ClientboundSetEntityLinkPacket(this, h))`. Then **if `this` is a passenger,
`stopRiding()`**: a leashed entity cannot ride.

`dropLeash()` = `dropLeash(e, send=true, dropItem=true)`; `removeLeash()` = `(true,
false)`; `setDelayedLeashHolderId(id)` (client) = new data with the id and `(false,
false)` (lines 90–103, 202–218).

`dropLeash(e, send, dropItem)` (lines 224–274): if data and holder non-null:
`setLeashData(null)`; `onLeashRemoved()` (no-op default); on the server: if `dropItem`,
`e.spawnAtLocation(level, Items.LEAD)` (at `e`'s position, `y + 0`; Entity.txt lines
6362–6433: `new ItemEntity(level, x, y, z, stack)` with its draws, N4 brief 8.4,
`setDefaultPickUpDelay`, `addFreshEntity`); if `send`,
`sendToTrackingPlayers(e, ClientboundSetEntityLinkPacket(e, null))`; then
`oldHolder.notifyLeasheeRemoved(e)`. No sound in `dropLeash` itself.

`AbstractBoat.remove(reason)` (AbstractBoat.txt lines 2058–2075): on the server, if
`reason.shouldDestroy()` and leashed, `dropLeash()` first. A boat broken or killed drops
its lead; one unloaded keeps its leash in its save. A boat that *holds* leashees and is
broken lets them go on their next tick (7.2, with the lead if `entityDrops`).
`Entity.removeAfterChangingDimensions` calls `removeLeash()` on a leashee that changes
dimension (Entity.txt lines 9228–9242).

**`ClientboundSetEntityLinkPacket`** (play, clientbound, id **102**, `minecraft:
set_entity_link` in `generated/reports/packets.json`): the leashee's id and the holder's
id, 0 for none. **Inferred** (the packet class not read): two `int`s written with
`writeInt` (not varints), source then destination. Sent:
- on a change: by `setLeashedTo` (with the holder) and `dropLeash`/`removeLeash` (with
  none), to the players tracking the leashee (`sendToTrackingPlayers`, not the leashee);
- at pairing: `ServerEntity.sendPairingData` (ServerEntity.txt lines 1001–1022), after
  `SetPassengers` (of the entity, then of its vehicle), if the entity is `Leashable` and
  leashed, `SetEntityLink(entity, holder)`.
**Client** (Leashable.txt lines 904–934): the client keeps the holder's id and looks it up
every time it needs the holder, so a link to a holder it is not yet shown resolves when
the holder appears; the edge need not order the holder's spawn first.

### 7.6 `LeadItem` and `LeashFenceKnotEntity`

**`LeadItem.useOn(ctx)`** (LeadItem.txt lines 10–40): if the clicked block is in
`#minecraft:fences` (`#minecraft:wooden_fences` and `nether_brick_fence`; the repo's
`clustine_blocks::data::FENCES`), on the server with a player:
`bindPlayerMobs(player, level, pos)`; else `PASS`. The lead in hand is never consumed:
the leashees move from the player to the knot. (**Inferred**: using a fence with any item or none
also binds; `FenceBlock` not read.)

**`bindPlayerMobs(player, level, pos)`** (lines 42–108):
```
list = leashableInArea(level, Vec3.atCenterOf(pos), l -> l.getLeashHolder() == player)
if list.isEmpty(): return PASS
found = LeashFenceKnotEntity.getKnot(level, pos)
knot = found.orElseGet(() -> createKnot(level, pos))       // added to the level now
any = false
for l in list: if l.canHaveALeashAttachedTo(knot): l.setLeashedTo(knot, true); any = true
if any: knot.playPlacementSound(); level.gameEvent(BLOCK_ATTACH, pos, player); return SUCCESS_SERVER
if found.isEmpty(): knot.discard()
return PASS
```
The search box is ±16 around the fence's centre; each must be within 12 (box centres)
of the knot. **Inferred** (`ChunkMap` not read): a knot made and discarded in the same
call may still reach clients as a spawn and a removal, since adding an entity pairs it
with viewers at once.

**`LeashFenceKnotEntity`** (LeashFenceKnotEntity.txt lines 1–393; a
`BlockAttachedEntity`, section 6 has the base):
- **Construction** `(level, pos)` (13–31): type `LEASH_KNOT`, `pos`, then `setPos(x, y,
  z)` of the block's integer corner, which (`BlockAttachedEntity.setPos`, BlockAttached
  Entity.txt lines 325–338) sets `pos = BlockPos.containing(...)`,
  `recalculateBoundingBox()` and `needsSync = true`.
- **Position and box** (`recalculateBoundingBox`, 37–98): position `(x + 0.5, y +
  0.375, z + 0.5)` (`OFFSET_Y = 0.375`), box `[x+0.5 ∓ 0.1875, y+0.375, z+0.5 ∓ 0.1875]`
  to height `+0.5` (type width 0.375 halved, height 0.5, both `f2d`). Size 0.375 × 0.5,
  eye height 0.0625 (generated `entity_types.rs`).
- **Synched data**: none of its own (`defineSynchedData` empty, 33–35); only
  `Entity`'s.
- **Saved**: nothing of its own (`addAdditionalSaveData` empty, 120–126); `block_pos`
  from `BlockAttachedEntity` (lines 280–316: read only if within 16 of its
  `blockPosition()`, else logged and ignored), and the usual entity fields.
- **Survives** (246–255): the block at `pos` is in `#minecraft:fences`. Checked by
  `BlockAttachedEntity.tick` (BlockAttachedEntity.txt lines 34–73): on the server,
  `checkBelowWorld()`, then `if (ticksSinceLastCheck++ >= 100)` (the old value is
  compared: **every 101 ticks**, the first on its 101st tick): reset to 0,
  `tickAtCheckInterval()` (no-op), and if not removed and not surviving: `discard()` and
  `dropItem(level, null)`. No other tick: no `baseTick`, no gravity, no movement.
- **`dropItem`** (111–118): only `playSound(LEAD_UNTIED, 1, 1)`. A knot never drops an
  item; leads come from the leashees (7.2).
- **Interaction** (128–234), on the server:
  1. if the hand holds shears: `r = Entity.interact(...)` (7.4: only the shears branch
     applies; it drops every leash the knot holds, each with a lead, `SHEARS_SNIP`,
     damages the shears); if `r` is a `Success` that `wasItemInteraction()`, return it
     (**Inferred**: `SUCCESS` counts as an item interaction; `InteractionResult$Success`
     not read).
  2. every leashable the **player** holds (`leashableLeashedTo(player)`, ±16 around the
     player) that `canHaveALeashAttachedTo(knot)` moves to the knot;
  3. if none moved and the player is **not** sneaking: every leashable the knot holds
     that `canHaveALeashAttachedTo(player)` moves to the player;
  4. if any moved in 2 or 3: game event `BLOCK_ATTACH` (the player),
     `playSound(LEAD_TIED)`, `SUCCESS`; else `Entity.interact(...)` (which for a knot,
     not being `Leashable`, gives `PASS` or the shears result).
  No lead item is taken or given in any of these.
- **`notifyLeasheeRemoved(l)`** (236–244): if `leashableLeashedTo(this)` is now empty,
  `discard()` (silently: no sound, no drop). Called when a leashee's leash drops or moves
  to another holder.
- **`getKnot(level, pos)`** (269–331): the first `LeashFenceKnotEntity` in the box
  `[x-1, y-1, z-1, x+1, y+1, z+1]` whose `getPos()` equals `pos`. `getOrCreateKnot`
  (257–267) and `createKnot` (333–346: `new LeashFenceKnotEntity(level, pos)`,
  `level.addFreshEntity`).
- **Breaking**: `hurtServer` (BlockAttachedEntity.txt lines 154–191): invulnerable to the
  source → false; `mobGriefing` off and the source a `Mob` → false; if not removed:
  `kill(level, source entity)` (`remove(KILLED)`, game event `ENTITY_DIE`), `markHurt()`,
  `dropItem` (the sound). A player's attack goes through `skipAttackInteraction`
  (86–112): if the player `!level.mayInteract(player, pos)`, the attack is skipped;
  otherwise `hurtOrSimulate(playerAttack(player), 0f)`, so one hit breaks it in any mode.
  Any `move` with a non-zero vector or `push` with a non-zero vector on the server also
  kills it and plays the sound (215–278). A lightning strike does nothing (346–348).
- **Pick result** (379–385): a lead (creative pick-block).
- **Add-entity** (357–366): `ClientboundAddEntityPacket(this, 0, getPos())`: data 0 and
  the block position. **Inferred** (the packet class not read): that constructor sends
  the block's integer corner as the position (the client places the knot itself at
  `+0.5, +0.375, +0.5`), with zero motion and the entity's rotations.
- Tracking: range 10 chunks, update interval `Integer.MAX_VALUE`, `trackDeltas` false
  (generated `entity_types.rs`, type id 77).
- `repositionEntityAfterLoad()` is false (BlockAttachedEntity.txt lines 320–323).
- `shouldRenderAtSqrDistance` < 1024 (**Client**).


---------------------------------------------------------------------------------------

## 8. The entity types of N5

### 8.1 Types, sizes and tracking

From `crates/clustine-data/src/generated/entity_types.rs` (the extracted `EntityType`
data; its index is the protocol id, checked equal to `generated/reports/registries.json`
for all 161 types). "Range" is `clientTrackingRange` in chunks, "interval" `updateInterval`
in ticks (`ServerEntity`'s gate, `test(t) = t % n == 0`; `i32::MAX` means, **Inferred**
(`UpdateInterval` not read), only at the first tick and on `needsSync` or dirty data).

| type | id | width × height | eye | range | interval | deltas | gravity (default) |
|---|---|---|---|---|---|---|---|
| `minecart` | 86 | 0.98 × 0.7 | 0.595 | 8 | 3 | true | 0.04, 0.005 in water (3.15) |
| `chest_minecart` | 25 | same | same | 8 | 3 | true | same |
| `furnace_minecart` | 57 | same | same | 8 | 3 | true | same |
| `tnt_minecart` | 137 | same | same | 8 | 3 | true | same |
| `hopper_minecart` | 66 | same | same | 8 | 3 | true | same |
| `spawner_minecart` | 125 | same | same | 8 | 3 | true | same |
| `command_block_minecart` | 29 | same | same | 8 | 3 | true | same |
| boats: `acacia` 0, `birch` 12, `cherry` 23, `dark_oak` 34, `jungle` 75, `mangrove` 82, `oak` 90, `pale_oak` 95, `poplar` 106, `spruce` 128 (`_boat`); `bamboo_raft` 9 | | 1.375 × 0.5625 | 0.5625 | 10 | 3 | true | 0.04 (4.4) |
| chest boats: the same woods' `_chest_boat` 1, 13, 24, 35, 76, 83, 91, 96, 107, 129; `bamboo_chest_raft` 8 | | 1.375 × 0.5625 | 0.5625 | 10 | 3 | true | 0.04 |
| `armor_stand` | 5 | 0.5 × 1.975 (small 0.25 × 0.9875; marker 0 × 0) | 1.7775 | 10 | 3 | true | `LivingEntity`'s (not read) |
| `item_frame` | 74 | 0.5 × 0.5 (the box is the frame's own, 6.5) | 0 | 10 | `i32::MAX` | false | none (never moves) |
| `glow_item_frame` | 61 | same | 0 | 10 | `i32::MAX` | false | none |
| `painting` | 94 | 0.5 × 0.5 (the box is the variant's, 6.4) | 0.425 | 10 | `i32::MAX` | false | none |
| `leash_knot` | 77 | 0.375 × 0.5 | 0.0625 | 10 | `i32::MAX` | false | none |
| `cushion` | 33 | 1.0 × 0.25 | 0.2125 | 10 | `i32::MAX` | false | none |
| `mannequin` (N7) | 84 | 0.6 × 1.8 | 1.62 | 32 | 2 | true | `LivingEntity`'s |
| `player` (as a rider) | 159 | 0.6 × 1.8 | 1.62 | 32 | 2 | false | |

None of these is fire immune. The boats' item ids are in 8.4. **Inferred** (`ChunkMap$TrackedEntity` not
read): an entity's effective tracking range is the largest of its own and its indirect
passengers', so a boat or cart with a player in it is tracked at 32 chunks' range
capped by the view distance.

Attachment points are in `EntityType` (not read). For orientation only, SteelMC's
extraction gives: passenger `(0, 0.1875, 0)` for every minecart, `(0, 0.5625, 0)` for the
boats (unused: `AbstractBoat` overrides `getPassengerAttachmentPoint`, 4.6), `(0, 1.975,
0)` for the armour stand, `(0, 0.25, 0)` for the cushion; the player's **vehicle**
attachment `(0, 0.6, 0)`, every N5 type's `(0, 0, 0)`. A rider's position is
`vehicle.position + passengerPoint(index, rotated by the vehicle's yaw) −
rider.vehicleAttachmentPoint` (1.5).

### 8.2 Synched data

Every type starts with `Entity`'s eight (1.11 has the per-tick sending; Entity.txt
12053–12110 the ids; the order of `define` is 0, 1, 3, 2, 4, 5, 6, 7, the ids by
`defineId`):

| id | accessor | serializer | default |
|---|---|---|---|
| 0 | `DATA_SHARED_FLAGS_ID` | `BYTE` | 0 (bit 0 on fire, 1 sneaking, 3 sprinting, 4 swimming, 5 invisible, 6 glowing, 7 fall flying) |
| 1 | `DATA_AIR_SUPPLY_ID` | `INT` | 300 |
| 2 | `DATA_CUSTOM_NAME` | `OPTIONAL_COMPONENT` | empty |
| 3 | `DATA_CUSTOM_NAME_VISIBLE` | `BOOLEAN` | false |
| 4 | `DATA_SILENT` | `BOOLEAN` | false |
| 5 | `DATA_NO_GRAVITY` | `BOOLEAN` | false |
| 6 | `DATA_POSE` | `POSE` | `STANDING` |
| 7 | `DATA_TICKS_FROZEN` | `INT` | 0 |

Then, per class (ids continue from the parent's):

| class (types) | id | accessor | serializer | default | where |
|---|---|---|---|---|---|
| `VehicleEntity` (carts, boats) | 8 | `DATA_ID_HURT` | `INT` | 0 | 3.14 |
| | 9 | `DATA_ID_HURTDIR` | `INT` | 1 | |
| | 10 | `DATA_ID_DAMAGE` | `FLOAT` | 0.0 | |
| `AbstractMinecart` | 11 | `DATA_ID_CUSTOM_DISPLAY_BLOCK` | `OPTIONAL_BLOCK_STATE` | empty | 3.15 |
| | 12 | `DATA_ID_DISPLAY_OFFSET` | `INT` | the kind's `getDefaultDisplayOffset()`: 6; chest 8; hopper 1 | |
| `MinecartFurnace` | 13 | `DATA_ID_FUEL` | `BOOLEAN` | false | 3.17 |
| `MinecartCommandBlock` | 13 | `DATA_ID_COMMAND_NAME` | `STRING` | "" | 3.19 |
| | 14 | `DATA_ID_LAST_OUTPUT` | `COMPONENT` | empty | |
| `AbstractBoat` (all boats and rafts) | 11 | `DATA_ID_PADDLE_LEFT` | `BOOLEAN` | false | 4.8 |
| | 12 | `DATA_ID_PADDLE_RIGHT` | `BOOLEAN` | false | |
| | 13 | `DATA_ID_BUBBLE_TIME` | `INT` | 0 | |
| `LivingEntity` (stand, mannequin; **not read**, SteelMC's list) | 8–14 | living flags `BYTE`, health `FLOAT`, effect particles `PARTICLES`, ambience `BOOLEAN`, arrows `INT`, stingers `INT`, sleeping pos `OPTIONAL_BLOCK_POS` | | | 5.6 |
| `ArmorStand` | 15 | `DATA_CLIENT_FLAGS` | `BYTE` | 0 (1 small, 4 arms, 8 no base plate, 16 marker) | 5.2, 5.6 |
| | 16–21 | head, body, left arm, right arm, left leg, right leg pose | `ROTATIONS` | (0,0,0), (0,0,0), (-10,0,-10), (-15,0,10), (-1,0,-1), (1,0,1) | |
| `Avatar` (mannequin; **not read**) | 15, 16 | main hand `HUMANOID_ARM`, model parts `BYTE` | | | 5.9 |
| `Mannequin` | 17 | `DATA_PROFILE` | `RESOLVABLE_PROFILE` | empty static profile | 5.9 |
| | 18 | `DATA_IMMOVABLE` | `BOOLEAN` | false | |
| | 19 | `DATA_DESCRIPTION` | `OPTIONAL_COMPONENT` | "NPC" label | |
| `HangingEntity` (frames, painting) | 8 | `DATA_DIRECTION` | `DIRECTION` | `SOUTH` | 6.2 |
| `ItemFrame` (both frames) | 9 | `DATA_ITEM` | `ITEM_STACK` | empty | 6.5 |
| | 10 | `DATA_ROTATION` | `INT` | 0 | |
| `Painting` | 9 | `DATA_PAINTING_VARIANT_ID` | `PAINTING_VARIANT` | `VariantUtils.getAny` (**Inferred** `alban`) | 6.4 |
| `Cushion` | 8 | `DATA_COLOR` | `DYE_COLOR` | `WHITE` | 5.8 |
| `LeashFenceKnotEntity` | — | none of its own | | | 7.6 |

`SetEntityData` writes the serializer's id from `EntityDataSerializers`' registration
order (not read). The repo has `BYTE` 0, `ITEM_STACK` 7, `BLOCK_POS` 10. For
orientation only, SteelMC's 26.3 list: `BYTE` 0, `INT` 1, `LONG` 2, `FLOAT` 3, `STRING`
4, `COMPONENT` 5, `OPTIONAL_COMPONENT` 6, `ITEM_STACK` 7, `BOOLEAN` 8, `ROTATIONS` 9,
`BLOCK_POS` 10, `OPTIONAL_BLOCK_POS` 11, `DIRECTION` 12, `OPTIONAL_LIVING_ENTITY_REFERENCE`
13, `BLOCK_STATE` 14, `OPTIONAL_BLOCK_STATE` 15, `PARTICLE` 16, `PARTICLES` 17, `POSE` 20,
`PAINTING_VARIANT` 34, `RESOLVABLE_PROFILE` 41, `HUMANOID_ARM` 42, `DYE_COLOR` 43. Each
must be checked against the official server (the comparisons) before it is relied on.

### 8.3 What `ServerEntity` does with each

- **Carts and boats** (interval 3, deltas): the n4-brief section 9 path, every third
  tick or on `needsSync`/dirty data; a boat a client drives is sent to the other
  trackers the same way, from the position `handleMoveVehicle` set. Hit data (8–10)
  makes the data dirty each tick it decays, so a shaking vehicle is sent every tick.
  Their client interpolation is `LinearInterpolationHandler` (3 steps for boats,
  **Client**); what its server-side tracker gives the `VecDelta`'s step count is not read.
- **A rider** of any of them: rotation only (1.11.1).
- **Frames, paintings, knots, cushions** (interval `i32::MAX`, no deltas): sent at
  pairing; again only on `needsSync` (set by `setPos`) or dirty data (an item put in, a
  rotation, a colour). An item frame also runs its map's updates every 10 ticks (maps:
  later). A painting's codec base is its block's corner.
- **Armour stands** (interval 3, deltas): as carts, plus a `LivingEntity`'s
  `UpdateAttributes` and `SetEquipment` at pairing and on change (1.11.2).
- **Pairing order** (1.11.2): `AddEntity` (hanging entities and knots with their own
  data and the block's corner), `SetEntityData` (non-default values), `UpdateAttributes`,
  `SetEquipment`, `SetPassengers` (own), `SetPassengers` (its vehicle's), `SetEntityLink`
  (if leashed), all in one bundle.

### 8.4 Boat items

The entity types and the items of the same name (`reports/registries.json`), each
stacking to 1 (`#minecraft:boat` holds the eleven non-chest types):

| entity | id | item | | entity | id | item |
|---|---|---|---|---|---|---|
| acacia_boat | 0 | 983 | | acacia_chest_boat | 1 | 984 |
| bamboo_raft | 9 | 995 | | bamboo_chest_raft | 8 | 996 |
| birch_boat | 12 | 979 | | birch_chest_boat | 13 | 980 |
| cherry_boat | 23 | 985 | | cherry_chest_boat | 24 | 986 |
| dark_oak_boat | 34 | 987 | | dark_oak_chest_boat | 35 | 988 |
| jungle_boat | 75 | 981 | | jungle_chest_boat | 76 | 982 |
| mangrove_boat | 82 | 991 | | mangrove_chest_boat | 83 | 992 |
| oak_boat | 90 | 975 | | oak_chest_boat | 91 | 976 |
| pale_oak_boat | 95 | 989 | | pale_oak_chest_boat | 96 | 990 |
| poplar_boat | 106 | 993 | | poplar_chest_boat | 107 | 994 |
| spruce_boat | 128 | 977 | | spruce_chest_boat | 129 | 978 |

Rafts differ from boats only in their ride height (0.5 against 0.1875, 4.6).

---------------------------------------------------------------------------------------

## 9. Random draws

Who draws, from which random, and how much. "Entity's" is the entity's own
`RandomSource` (`Entity.random`, made in `Entity.<init>`), "level's" the level's. In
Clustine (ADR-0028 section 4) each becomes a named stream; the plan names them.

| where | random | draws | section |
|---|---|---|---|
| any new entity (`Entity.<init>`): a placed cart, boat, stand, frame, painting, knot, cushion, a dropped item | the new entity's | two `nextLong` (UUID) | n4 8.4 |
| `Painting.create` | the new painting's | one `nextInt(n)` after its UUID, `n` the count of placeable variants of the largest fitting area (**Inferred** `Util.getRandomSafe`); drawn even when the item names a variant | 6.4 |
| `ArmorStandItem.useOn` → `EntityType.create` | level's | one `nextFloat` for a yaw that `snapTo` overwrites (**Inferred**, `EntityType` not read): name it, draw nothing | 5.1 |
| a vehicle's own item dropped (`destroy` → `spawnAtLocation`), a lead dropped (`dropLeash`, the 100-tick restore), a cushion's item, a painting's or frame's item | the new `ItemEntity`'s | two `nextLong`, `nextFloat` (bob), `nextFloat` (yaw), two `nextDouble` (motion); no position draws | n4 8.4 |
| a frame's held item dropped | the frame's | one `nextFloat` against `dropChance`, after the frame's own drop | 6.5 |
| an armour stand broken by a player (`popResource` of the stand item) and each worn item (`popResource` at the block above) | level's | the offset draws of `popResource`, then the item's own | 5.4 |
| `dropAllDeathLoot` of a stand | the `minecraft:entities/armor_stand` sequence | none expected (empty table; **Inferred**) | 5.4 |
| container contents spilled (`Containers.dropContents`, not read) | level's | **Inferred**, per non-empty slot: three `nextDouble` for the position, then per split `nextInt(21) + 10` and three `triangle` (two `nextDouble` each) for the motion; then each item's own | 3.18 |
| `MinecartFurnace.tick` | entity's | `nextInt(4)` every fuelled tick, server too (an unsent particle) | 3.17 |
| `Entity.lavaHurt` (a cart in lava) | entity's | `nextFloat` for the burn sound's pitch, when damage was taken | 3.3 |
| boat paddle sound | boat's | `nextFloat` (pitch) per sound; ticks 1, 2, 17, 33, … of a stroke | 4.2 |
| `AbstractBoat.onAboveBubbleColumn` | boat's | `nextInt(100)` per call (twice a tick, 4.0 item 8) when not under water; on 0, three `nextFloat` (pitch, particle x, z) | 4.7 |
| `Entity.baseTick`'s water splash (boats) | entity's | as n4 | n4 |
| every `level.playSound(null, …)` (placing, breaking, sitting, leads, shears, paddles, frame items) | level's | **Inferred** one `nextLong` for the sound packet's seed (`ServerLevel.playSeededSound` not read) | |
| `MinecartTNT` (N6) | entity's | `nextInt(20)` twice (fuse), `nextDouble` (power) | 3.19 |
| `ContainerEntity` loot tables (not in N5) | the seed's, or level's | the table's | 3.18 |

None in: `startRiding`, passengers, `rideTick`, `positionRider`, dismounting,
`DismountHelper`, `push`, saving, `ServerEntity`, `ServerGamePacketListenerImpl`'s paths
of section 2, `MinecartItem`, `BoatItem`, `OldMinecartBehavior`, `DetectorRailBlock`,
`VehicleEntity.hurtServer`, `tickLeash` and the wrench, `LeadItem`, the knot,
`HangingEntityItem` (beyond the new entity's), item frame interaction.

---------------------------------------------------------------------------------------

## 10. What touches the line between regions

### 10.1 Units, references and order

- **The unit** is the root vehicle and its indirect passengers (ADR-0028): references by
  identity in the game, keys inside one record in Clustine. The passengers' order is
  state (0, item 9). A cart may be the root of a boat (0, item 15). `getRootVehicle`,
  `isPassengerOfSameVehicle` (a unit never pushes itself) stay within a unit.
- **Rider placement** reads only the vehicle. A rider's own `tick` (inside `rideTick`)
  reads blocks around the rider, within the copies of the root's chunk.
- **The leash holder** is a key (a player, a boat, a knot) or, as saved, a fence
  position. A held leash is at most 12 between box centres and the binding searches are
  ±16: inside the copies' two chunks, so a holder beyond the copies is beyond 12 and the
  game's own snap applies (ADR-0028's rule is the game's). A holder's removal (a knot
  broken, a player leaving) is seen through the copies, at worst a tick late. A
  restore by UUID must look up by key across the copies and the players the region
  knows of, for 100 ticks; a restore by fence position that creates a knot in another
  region's chunk is a spawn across the line, and the leashee needs the knot's key at
  once. Binding at a fence changes the holder of leashees in up to ±16, possibly in
  several regions' chunks; a knot discarding itself when its last leashee goes is a
  write to the holder.
- **A player holding a leashed boat** crosses by their own hand-over while the boat
  stays: the leash must not drop at the hand-over.
- **The comparator and the frame it reads** may be in different regions; the frame's
  region tells the comparator's chunk of a change as a neighbour update across the line
  (6.6).

### 10.2 Riding across a line (ADR-0028 section 5)

- **The connection's vehicle state** (0, item 12) must travel with a rider's hand-over
  or be seeded by the new region before the kept `MoveVehicle` is applied.
- **`MoveVehicle` for a root that has just crossed** goes to the region that now holds
  the root. Its accepted position is the client's own (`absSnapTo`), so the root can end a
  packet in another chunk than its tick left it in: the crossing test is made after
  packets as after the tick.
- **Seat order on arrival**: a pending player seated later is inserted by `addPassenger`'s
  rule (index 0 if the first is not a player).
- **The player's view** is updated by the player's position (`chunkSource.move(player)`),
  which lags the vehicle by a tick; the edge's view follows the player's position.
- **Interact and Attack** name an entity by network id within about 8 blocks: a boat or
  a stand held by another region. Mounting it makes the player's holding chunk the
  root's (a hand-over); hitting or using it is an effect on an entity across a line
  (ADR-0028 section 7), carrying the amount, whether the attacker is creative
  (`instabuild`), the hand's item and, for the furnace cart, the player's position.
- **Dismounting** reads blocks within one block of the vehicle (the cart's eight
  neighbours from one below to one above and a column above; the boat's escape point):
  inside the copies.
- **Ejection** (an activator rail, a boat under water for 60 ticks, a downward bubble
  column, `setLeashedTo`) dismounts in the vehicle's region; for a player it is a
  dismount the edge shows.
- **Saving** (0, item 10): what a player's logout does to the boat they sit in.

### 10.3 Blocks read a tick away

- **Minecarts**: the block at the cart and the one below (`getCurrentBlockPosOrRailBelow`),
  `getPos` before and after the move (possibly the next block), on a powered rail at rest
  its four neighbours (conductors), the collision around the box and its move, the blocks
  in its box (detector rails). All within one block; the old behaviour moves at most 0.4
  per axis a tick plus the snap, never more than about a block: a cart never skips a
  chunk. Falling off rails it is not capped in `y` (about 0.76 a tick at most,
  **Inferred**). Under the experiment it would walk rails up to `max_minecart_speed`/20
  blocks ahead (3.9).
- **Boats**: `getGroundFriction` reads one block beyond the box's floor in x and z (up
  to two blocks from the centre), the status the cells of the box; inside the copies.
  A driven boat's move is limited only by "too quickly" (about 10 blocks plus its
  motion), and several `MoveVehicle` a tick are allowed: its move check can read a chunk
  neither loaded nor copied, read as the game reads a missing chunk for collisions
  (ADR-0028's fast entities).
- **Hanging entities**: a painting's support box covers up to four blocks along the wall
  and four up, and the block behind any hanging entity is in the next chunk when the wall
  is the chunk's edge; read every 101 ticks and at placing. A cushion's box (1.0 wide,
  free position) can straddle a chunk line.
- **Knots**: the fence at their own block.

### 10.4 Entities read

- A cart's push and pick-up box (its box + 0.2 horizontally), a boat's (+ 0.2, − 0.01
  vertically), the entity colliders of their moves, `BoatItem`'s check of entities whose
  boxes hold the player's eyes, `ArmorStandItem`'s `getEntities` in the stand's box, the
  stand's `pushEntities` of rideable carts, `canCoexist`'s other hanging entities in a
  painting's or frame's box (`Painting.create` runs it for every placeable variant), the
  detector rail's carts, `leashableInArea` (±16). Each may reach another region's
  chunk: the copies must carry entities, and a push, a pick-up (a mount) or a leash's
  holder change there is an effect on another region's entity (ADR-0028 section 7).
- **Carts pushing carts** change the other's motion within the tick: the outcome depends
  on the order carts tick (the game's entity list; ADR-0028's by key): a named
  difference judged by distribution (3.10).
- **Two paintings placed** into the same wall from both sides of a line in one tick can
  each see the other's place free (**Inferred**: until the next 101-tick check pops one;
  the game would have refused the second). A named difference.

### 10.5 What an arrival must carry beyond the saved fields

The game saves none of these, yet a player would notice their loss at a line within
minutes or seconds: a vehicle's `damage`, hurt time and direction (3.0 item 11); the
passenger's `boardingCooldown` (1.0 item 6); a boat's `outOfControlTicks`,
`paddlePositions`, `deltaRotation`, status, bubble time and its flags; a cart's
`lastKnownPosition` (for `getKnownMovement`); a leash's `angularMomentum`. A stand's `lastHit` is
against game time (0, item 23); hanging entities' `ticksSinceLastCheck` may restart at 0
at a take-in, as at the game's load. Large arrivals: a chest cart or chest boat with 27
stacks can exceed a log record (ADR-0028, "Large").

### 10.6 Drops and spawns

Drops are born at the entity's position: a vehicle's item at its exact position, a
hanging entity's 0.15 out from the wall, the stand's at its block and the block above,
container contents in the vehicle's block column (**Inferred**, `Containers`): all in the
entity's own chunk, which for a painting can differ from its `block_pos`'s (0, item 32).
A painting, frame, cart or boat is filed in the chunk of its position.

---------------------------------------------------------------------------------------

## 11. Not read

Each is a class (or method) whose bytecode a rule above depends on; the sections that
mark **Inferred** rest on these. Grouped by what they are needed for.

**Riding and the player**
- Not read: `net.minecraft.world.entity.LivingEntity`, needed for `stopRiding`/
  `removeVehicle`/`dismountVehicle` (where a dismounted player or mob is put),
  `getDismountPoses`, `getLocalBoundsForPose`, `isClientAuthoritative`, `rideTick`,
  `canRide`, `pushEntities`/`doPush`/`isPushable`, `swing` (`animate` or
  `swing_animation`), and all of the armour stand's tick, `aiStep`, `travel`, health,
  equipment (`setItemSlot`, `getEquipmentSlotForItem`, `SetEquipment`), attributes,
  `interact` (name tags), `dropAllDeathLoot`, `getScale`, `getAgeScale`,
  `isInvulnerableTo`, its synched data ids 8–14.
- Not read: `net.minecraft.world.entity.player.Player`, needed for `removeVehicle`,
  `rideTick`, `wantsToStopRiding`, `canRide`, `DEFAULT_VEHICLE_ATTACHMENT`,
  `isClientAuthoritative`, `interactOn` (the order of `interact` and
  `interactLivingEntity`, a creative player's count kept), `attack` (the amount a hit
  deals, the path to `hurtOrSimulate` and `skipAttackInteraction`),
  `isWithinEntityInteractionRange`, `isWithinAttackRange`, `isSecondaryUseActive`,
  `getKnownMovement`, `mayUseItemAt`.
- Not read: `net.minecraft.server.level.ServerPlayer`, needed for `startRiding`,
  `stopRiding`, `dismountTo`, `teleportTo` (what a player is sent on mounting and
  dismounting), `doTick`/`rideTick` (the shift dismount), `setLastClientInput` and
  `getLastClientMoveIntent` (the cart's input), `getControlledVehicle`,
  `checkMovementStatistics`, `setKnownMovement`, saving the root vehicle with the
  player, and how a server-side `push` on a player reaches its client.
- Not read: `net.minecraft.world.entity.Avatar`, needed for the mannequin's data ids 15–16.
- Not read: `net.minecraft.server.players.PlayerList`, needed for loading and saving a
  player's root vehicle (`UNLOADED_WITH_PLAYER` at logout; 0, item 10).
- Not read: `net.minecraft.server.level.ServerLevel`, needed for `tickNonPassenger` and
  `tickPassenger` (the order passengers tick), `getEntity(UUID)`, `playSeededSound` and
  `playSound` (the sound seed draw), `addParticle` (that server particles are dropped),
  `getGameTime`.
- Not read: `net.minecraft.server.level.ChunkMap` and `ChunkMap$TrackedEntity`, needed
  for the effective tracking range of a vehicle with passengers and when a new entity
  (a knot made and discarded in one call) is paired with viewers.
- Not read: `net.minecraft.server.level.ServerChunkCache.move`, for completeness only.

**Types, data and packets**
- Not read: `net.minecraft.world.entity.EntityType` (its static initialiser's builder
  calls), `EntityAttachments`, `EntityAttachment`, needed for the passenger and vehicle
  attachment points of every N5 type and the player, the player type's
  `canSerialize`/`noSave`, `isBlockDangerous`, `loadEntityRecursive` (how a saved unit is
  rebuilt), `create(level, config, pos, reason, …)` and `getYOffset` (an armour stand's
  spawn height and its discarded yaw draw), `createDefaultStackConfig` (what a placed
  entity takes from its stack: custom name, `entity_data`, implicit components), and the
  boats' drop items.
- Not read: `net.minecraft.network.syncher.EntityDataSerializers`, needed for the wire
  ids of every serializer in 8.2 and the encodings of `OPTIONAL_BLOCK_STATE`,
  `DIRECTION`, `PAINTING_VARIANT`, `ROTATIONS`, `DYE_COLOR`.
- Not read: the packet classes `ServerboundInteractPacket`, `ServerboundAttackPacket`,
  `ServerboundMoveVehiclePacket`, `ClientboundMoveVehiclePacket`,
  `ServerboundPaddleBoatPacket`, `ServerboundPlayerCommandPacket` (and its `Action`),
  `ClientboundSetPassengersPacket`, `ClientboundSetEntityLinkPacket` (**Inferred** two
  `int`s), and `ClientboundAddEntityPacket.<init>(Entity, int, BlockPos)` (the position
  and motion a hanging entity or knot is added with), needed for their codecs.
- Not read: `ServerGamePacketListenerImpl$1`, needed for the `player_command` action
  ordinals; and a `javap -v` of `ServerGamePacketListenerImpl` for the `invokedynamic`
  predicate of `noBlocksAround` (**Inferred** `isAir`).
- Not read: `InteractionResult$Success`, `$SwingSource`, `$ItemContext`, needed for
  `consumesAction`, `shouldSwing`, `wasItemInteraction`, `withoutItem`.
- Not read: `net.minecraft.world.entity.UpdateInterval`, needed for `test` and
  `nextInterval` with `Integer.MAX_VALUE`.
- Not read: `net.minecraft.world.entity.LinearInterpolationHandler` and
  `InterpolationTracker`, needed for the position paths (and step counts) `ServerEntity`
  sends for boats and old-behaviour carts, and the client's interpolation.

**World queries and shapes**
- Not read: `net.minecraft.world.level.CollisionGetter` / `EntityGetter` / `Level`
  (`noCollision(Entity, AABB)`, `getEntityCollisions`, `getPreMoveCollisions`,
  `noBlockCollision`, `noBorderCollision`, `getEntities`, `hasEntities`, `findBlocksIn`),
  needed for entity colliders, the refusal of a vehicle's move, the floating test, what
  blocks a placing, and `canCoexist`'s box test.
- Not read: `net.minecraft.world.level.BlockGetter.getBlockFloorHeight`, needed for every
  dismount search.
- Not read: `BlockMatcher` (`forEachUntil`, `allMatched`), needed for which blocks a
  painting's support box, a cushion's anchor and fire searches visit, and their results.
- Not read: `net.minecraft.world.entity.EntitySelector` (`pushableBy`, `CAN_BE_PICKED`,
  `CONTAINER_ENTITY_SELECTOR`), needed for what a cart or boat pushes or picks up, what
  blocks a boat's placing, which carts a detector rail counts.
- Not read: `net.minecraft.world.phys.Vec3` (`yRot`, `projectedOn`, `horizontal`,
  `relative`, `atCenterOf`, `multiply`, `normalize`, `distanceTo`) and `AABB.ofSize`,
  needed for the seats' turn, the furnace's push, the hanging boxes and the leash's
  attachment points bit for bit; `Mth.sin`/`Mth.cos` (the repo has the table: confirm
  its argument type).
- Not read: `Item.getPlayerPOVHitResult`, needed for the boat's ray (reach, clip mode).
- Not read: `BlockPlaceContext`, needed for the armour stand's clicked position.

**Items, drops and containers**
- Not read: `net.minecraft.world.Containers`, needed for how contents are spilled
  (positions, splits, motion, emptied stacks).
- Not read: `AbstractContainerMenu.getRedstoneSignalFromContainer`, needed for a
  container cart's slowdown and a detector rail's comparator output.
- Not read: `ContainerHelper`, needed for the `Items` list's format.
- Not read: `LootTable.fill`, needed for a container's loot-table seed (not in N5).
- Not read: `ItemStack` (`consume`, `set` with a null component), and
  `ServerPlayerGameMode.useItemOn`, needed for whether a creative player keeps a placed
  cart, boat, stand, frame, painting or the coal and frame item, and a drop's name when
  the entity has none.
- Not read: `DamageSource` (`isCreativePlayer`, `getEntity`, `getDirectEntity`), needed
  for the creative test of a hit and invulnerability.
- Not read: the cushion's placing item (`Items.CUSHION`, a `ColorCollection`; probably
  `CushionItem`), needed for where a cushion is placed, its use of `canBePlacedAt`, its
  sound and consumption.
- Not read: `EquipmentSlot`, needed for `DisabledSlots`' filter bits and the order worn
  items drop.
- Not read: `VariantUtils` and `Util.getRandomSafe`, needed for the painting's default
  variant, its NBT key and the exact draw.
- Not read: `GameRules` (`MAX_MINECART_SPEED`'s default), only if the experiment is kept.

**Blocks**
- Not read: `ComparatorBlock` (`getInputSignal`, `getItemFrame`) and
  `Level.updateNeighbourForOutputSignal`, needed for how a comparator reads a frame and
  which comparators a frame's change tells.
- Not read: `BubbleColumnBlock`, needed for when a boat is told it is above or inside a
  column.
- Not read: `FenceBlock`, needed for binding leashes by using a fence without a lead.

**Mobs (N7)**
- Not read: `Mob`, needed for the leash rules of mobs (`leashTooFarBehaviour`,
  `closeRangeLeashBehaviour`, `canBeLeashed`, `onLeashRemoved`, the 16-block limit) and to
  confirm no N5 class overrides them; `HappyGhast`, for the quad leash's holder.
