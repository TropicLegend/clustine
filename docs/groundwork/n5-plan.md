# Step N5: vehicles, riding and what a creative player places, planned

Draft of 2026-10-18, to be reviewed before it is built. Step N5 of
[ADR-0028](../adr/0028-entities.md) (sections 2, 5, 6 and 7 as accepted) and of the
[phase plan](n-plan.md): "minecarts on rails, boats driven by the client, riding, armour
stands, item frames, paintings and the other hanging entities, leashes". The rules of
the game are in the [porting brief](n5-brief.md), read from the 26.3 jar; this plan
cites it by section (brief 3.6, brief 0.12) and decides what the brief leaves open.

## 1. What a player meets after N5

- A minecart placed on a rail rolls down slopes, round curves and over powered rails,
  which push it, and stops on unpowered ones; a player rides it with a right click,
  speeds it by walking forwards, and gets off by sneaking, placed beside it as the game
  places them. An empty moving cart picks up a boat it runs into. Activator rails eject
  riders; detector rails give a signal while a cart is on them, and a comparator reads
  a chest cart's fill there. The furnace cart takes coal and pushes. Carts push each
  other and stop against players.
- A boat or a raft placed on water or land is driven by the client of the player in its
  front seat, as in the game, with paddles shown to others; a second player sits behind.
  A boat nobody drives floats, drifts with currents, rises and sinks in bubble columns,
  and picks up a mannequin (later, animals) it runs into. Players stand on boats; items
  land on them.
- An armour stand is placed, takes and gives the items a player puts on it, falls,
  floats, and breaks at once to a creative player's hit.
- Item frames and glow item frames are hung on any face, take an item and turn it by a
  click, show it to everyone, give a comparator their rotation two blocks away, and are
  popped by a piston or by losing their wall. Paintings are hung with the variant drawn
  as the game draws it. Cushions are placed and sat on.
- A lead ties a boat to a player, to a fence (a knot) or to another boat; the boat is
  pulled as the game pulls it, and the lead snaps beyond 12 blocks.
- Every one of these crosses a line between regions as an item does (N3), and a player
  riding a boat or a cart across a line goes on riding, with their vehicle and anyone
  else in it, without a jump back, a flicker or a dismount.
- A player who leaves while sitting alone in a boat takes the boat with them and comes
  back in it, as in the game (brief 0.10).

## 2. Decisions the brief leaves open (taken here; the owner can overrule)

1. **Minecarts move by `OldMinecartBehavior`** (brief 0.1). The experiment
   `minecart_improvements` is not offered: Clustine has no data packs, and a world that
   names it is refused as one with an unknown feature is. `NewMinecartBehavior` is not
   ported.
2. **TNT and hopper minecarts are placed by their items, inert until N6**, which comes
   next: a TNT cart neither primes nor explodes, a hopper cart takes nothing in. Named
   for those days. Spawner and command-block carts are not placeable in creative from
   the inventory; one met in a world is kept as an unknown entity (ADR-0028, section 2).
3. **Containers do not open** (chest carts, chest boats, hopper carts), as no chest
   block opens yet (the parity matrix, "Inventory and containers"): their contents are
   kept, carried and spilled as the game spills them, and a click on one does what a
   click on a chest does now, nothing. The containers' menus are a later step with the
   chest's.
4. **An armour stand is a reduced `LivingEntity`** (brief 0.7, 5.5): its travel in air,
   water and lava, the zeroing of small motion, the fall check, air, freezing,
   equipment changes and `pushEntities`; no AI, effects, damage path but its own, death
   loot but its own. The reduced `LivingEntity` lives in `clustine-sim/src/living/`, so
   that N7 extends it rather than replaces it.
5. **`Mannequin` is not built** (brief 0.6): one in a world is kept as an unknown
   entity until N7. Boats and carts pick up nothing living until then.
6. **Game time for the stand's double hit** (brief 0.25) is the region's own tick: both
   hits are handled by the stand's holder, and `lastHit` is not saved in the game
   either. A stand crossing a line between the two hits needs two more: a named
   difference.
7. **A vehicle with exactly one player in it leaves with the player** (brief 0.10): the
   unit goes into the player's stay in the world store (ADR-0020) at logout and comes
   back with them, as the game's `RootVehicle`. With none or two or more players it
   stays in its chunk.
8. **Containers' contents in a record** are the stacks as N2 encodes them; a chest
   boat's 27 stacks fit one record (N2's large entity split covers them anyway).
9. **Unsaved state a player would notice at a line travels with the entity** (brief
   10.5): `Entity` gains a field of runtime state that commits and arrivals carry and
   chunk saves leave out, as the game leaves it out of NBT. A vehicle's damage, hurt time
   and direction, a passenger's boarding cooldown, a boat's out-of-control ticks, paddle
   phases, turning and status, a leash's angular momentum. Hanging entities' check
   counter starts again at a take-in, as at the game's load.
10. **Interact and Attack on an entity across a line** (brief 10.2) are effects on an
    entity (ADR-0028, section 7) when the entity's holder is not the player's region.
    In N5 that is: mounting (which becomes the player's hand-over, section 5 below), a
    hit on a vehicle, a stand, a frame, a painting, a cushion or a knot, and using a
    frame, a stand, a lead or a boat. The effect carries what the game reads of the
    player (creative or not, the item in hand, the attack strength, sprinting,
    critical, sneaking, the position for the furnace cart and the stand's slot height).
11. **Entity collision** (brief 0.18) is ported now: boats and carts collide with
    entities that are colliders or pushable, everyone collides with boats, and their
    boxes block placing. The client's own player box is the edge's to report (ADR-0005):
    the region uses the player's box as it knows it.
12. **The tracking range of a ridden vehicle** (brief 0.38) is the player's, 32 chunks,
    capped by each viewer's view distance; the edge widens a unit's range while a player
    rides it.

## 3. Parts, in order

Each part ends in a verified commit on `main`. The contracts (N5a) are mine and pushed
before anything is delegated; the riding across lines (N5h) and everything in the
edge's hand-over are mine (CLAUDE.md). Parts N5c to N5g are delegated, each to an agent
with crates or files of its own and a worktree, and the tests of all of them are written
afterwards from the record, the brief and this plan by someone who built none of it
(N5j).

### N5a, the contracts

- `clustine-world`: `EntityData` gains `Minecart { kind, behaviour state, contents,
  fuel, push, display block, offset }`, `Boat { wood, chest: Option<contents>, raft }`,
  `ArmorStand { pose, flags, equipment, … }`, `ItemFrame { item, rotation, drop chance,
  glow, fixed, invisible }`, `Painting { variant }`, `Cushion { colour }`, `LeashKnot`,
  each with the brief's saved fields (brief 3.13, 4.8, 5.7, 6.5, 6.4, 5.8, 7.6);
  `BlockAttached { block_pos, direction }` common to the hanging kinds. `Entity` gains
  `passengers: Vec<EntityId>` (in order, brief 0.9), `vehicle: Option<EntityId>`,
  `leash: Option<LeashHolder>` (`Entity(EntityId)`, `Player(Uuid)`, `Fence(BlockPos)`),
  and `runtime: RuntimeState` (decision 9). A unit lives in its root's chunk, every
  member a record of that chunk's entity file, as the record says.
- `clustine-format`: the codecs of each, the runtime state in commits and arrivals but
  not in chunk saves; `FORMAT_VERSION` raised, a world made before refused and made
  again as before.
- `clustine-sim` API: `PlayerInput::Interact { entity, hand, location, sneaking }`,
  `PlayerInput::Attack { entity }`, `PlayerInput::MoveVehicle { position, rotation,
  on_ground }`, `PlayerInput::PaddleBoat { left, right }`, `PlayerInput::Command(…)` for
  `player_command`'s actions that N5 needs (none to start; the sprint flag is already
  kept). `RegionEvent::Passengers { vehicle, passengers }`, `RegionEvent::Leashed {
  entity, holder }`, `RegionEvent::VehicleCorrected { player, position, rotation }` (the
  clientbound `move_vehicle` to the driver), `RegionEvent::Mounted`/`Dismounted` for the
  rider's own `player_position` and `set_passengers` (brief 0.11, 0.15), and the
  sounds the step makes (`sound_entity`).
- `clustine-rpc`: the player's transfer (`PlayerTransfer`) gains the vehicle's key, its
  chunk and the rider's place in the order (ADR-0028, section 5); a stay (ADR-0020)
  gains an optional unit (decision 7). Effects gain `Effected::OnEntity { key, what }`
  for decision 10, with `what` an enum of the uses and hits above.
- `clustine-protocol`: `interact`, `attack`, serverbound and clientbound `move_vehicle`,
  `paddle_boat`, `player_command`, `pick_item_from_entity`, `set_passengers`,
  `set_entity_link`, `sound_entity`, `swing_animation` (brief 2.11), and the entity data
  serializers N5 needs (brief 8.2). The relay of a player's own arm swing is checked
  against 26.3 (brief 0.37) and mended if it should be `swing_animation`.

### N5b, riding and entity collision (the sim, mine with an agent's help)

- `physics/`: entity colliders (`getEntityCollisions` over the region's entities and the
  copies' entities, brief 0.18), `blocksBuilding` for placing.
- `riding.rs` in the sim: `startRiding`, `addPassenger`/`removePassenger` with the
  game's order (brief 1.2–1.4), `rideTick` and `positionRider` with the attachment points
  (brief 1.5; the per-type points extracted by `cargo datagen`, brief 11), the level's
  passenger tick order (brief 0.15), dismounting through `DismountHelper` (brief 1.7),
  ejection, a riding player's moves taking only their rotation (brief 0.14), the shift
  dismount from the latest `player_input` or `interact` (brief 0.15), mounting and
  dismounting as teleports with what the rider's client is sent.
- The cushion as the first vehicle (brief 5.8): placing, sitting, sneaking off, the
  101-tick survival check, a hit breaking it. It proves riding before any vehicle moves.
- The edge (mine, as it touches the player's own state): the `Tracker`'s riding flag,
  `set_passengers` at pairing both ways and on every change, no position to a rider,
  the rebase after a dismount (brief 0.11), the widened range (decision 12).

### N5c, hanging entities (delegated)

Item frames, glow item frames and paintings (brief 6), knots' block attachment shared
with N5g: placing (`HangingEntityItem`, `ItemFrameItem`, the painting's draw, brief
0.34), the 101-tick survival check, any move or push breaking them (brief 0.33),
breaking and drops in creative and survival order (brief 0.26), the frame's item and
rotation, the comparator reading a frame two blocks away (brief 6.6) in `diodes.rs`, and
what clients are sent (the add-entity data with the direction, brief 0.32). Across a
line, the comparator's update to a frame's change is a neighbour update as any other
(phase E).

### N5d, armour stands (delegated)

The reduced `LivingEntity` of decision 4 in `living/`, the stand's placing (the two draws
thrown away, any non-spectator entity in the way refusing it, brief 0.25, 0.39), poses
and flags, interaction by slot height (brief 5.3), equipment and `SetEquipment`, the
double hit and the creative break dropping nothing (brief 0.25).

### N5e, minecarts and rails (delegated)

`MinecartItem` and the carts' construction (brief 3.1, 3.2), `AbstractMinecart.tick` and
`OldMinecartBehavior` on every rail shape, powered and activator rails, off-rail drag
(brief 3.3–3.7), riding (brief 3.12) and the rider's input (brief 0.3), pushing and
being pushed and the pick-up of pushable entities (brief 3.10, 0.16), the furnace cart
(brief 3.17), container carts' contents and their roll by fill (brief 0.28), breaking
(brief 3.14, 0.22, 0.23), detector rails' entity half and their comparator output (brief
3.8, 0.21), TNT and hopper carts inert (decision 2). Judged against the official server
too: the old behaviour draws nothing (brief 0.39), so a cart let go at the top of a
slope of rails is in the same place on the same tick in both, which a comparison in
`oracle` checks bit for bit.

### N5f, boats and rafts (delegated, with the edge's part mine)

`BoatItem` placing (brief 4.1), `AbstractBoat.tick` split by authority (brief 0.2, 4.2):
the server's floating, status, currents, bubble columns and pick-up for a boat nobody
drives (brief 4.3, 4.4, 4.7); a driven boat moved by `PlayerInput::MoveVehicle` through
the game's check, with the connection's state kept in the region's record of the player
(brief 0.12, 0.13, 2.3), the 160-tick floating kick, `paddle_boat` shown to others;
seats (brief 4.6), collisions (brief 4.9), breaking (brief 4.10), chest boats (decision
3), rafts. The edge relays `move_vehicle` and `paddle_boat` to the player's region as it
relays moves (mine).

### N5g, leashes (delegated, after N5f)

`Leashable` for boats (brief 7.1–7.3: the elastic pull and the wrench, the snap at 12),
`LeadItem` and knots (brief 7.6), taking, giving and cutting a leash (brief 7.4), the
restore by UUID or fence for 100 ticks (brief 0.36), `set_entity_link`. A player holding
a leashed boat crosses by their own hand-over and the leash holds (brief 10.1).

### N5h, units crossing lines and riders handed over (mine)

ADR-0028 section 5, "Vehicles and riders", as the brief amends it (section 4 below).

### N5i, measured

As N4 was: the log and the tick's length with 50 carts on a loop of rails and 20 boats
drifting, on the storage with its cache; the oracle comparison of carts (N5e).

### N5j, tests from the specification

By someone who built none of N5: from the record, the brief and this plan, with the
crashes of each process at each step of a rider's crossing.

## 4. How the record's "Vehicles and riders" is built, with the brief's amendments

1. **One unit, one chunk.** A unit is `Whole` or `Gone` together in its root's chunk
   whenever the root changes chunk; a passenger's own position never decides its chunk.
   A unit with no player crosses as one arrival carrying every member, in order.
2. **Seat order** is the order of `passengers` (brief 0.9). A player arriving to a unit
   whose first passenger is not a player goes first, as `addPassenger` puts them.
3. **A riding player's holding chunk** is their root's, in every judgement the record
   lists. The hand-over is made when the root changes chunk into land the region does
   not tick, to the root's new holder, carrying the vehicle's key, its chunk and the
   rider's place, and the rider's vehicle-move state (brief 0.12, 10.2) so that the new
   region applies the kept `MoveVehicle` as the old one would have, with no correction.
   The unit's arrival and the players' hand-overs go in the same commit.
4. **Seated, vehicle pending** (the record): a player who comes before their vehicle is
   kept with their latest `MoveVehicle`, sent nothing about the vehicle, and seated when
   the unit is taken; one whose unit has not come within 100 ticks is dismounted where
   they are and never seated later; the unit's region drops a pending player passenger
   not seated within the same 100 ticks or whose hand-over is discarded or passed over.
5. **A driven boat's move crossing in a packet**: an accepted `MoveVehicle` puts the
   root at the client's position (brief 10.2), which may be in another chunk; the region
   judges the crossing after each packet as after a tick.
6. **Logout in a boat alone** (decision 7): the unit leaves its chunk with the player's
   stay record, in the same commit, as the game's `UNLOADED_WITH_PLAYER`; at login it
   comes back where it was saved, the player seated. A login into land another region
   holds hands the unit over with the player.
7. **Effects on entities across a line** (decision 10): `Effected::OnEntity`, filed for
   the entity's chunk's user as effects are (ADR-0025), done in the tick it is due; one
   whose entity is gone by then does nothing. A mount through one becomes a hand-over of
   the player to the unit's region.

## 5. Named differences expected

- Carts pushing carts tick in the order of their keys, not the game's entity list: the
  outcome of a collision of three or more carts is the game's in distribution (brief
  10.4).
- Two paintings placed into one wall from both sides of a line in one tick may both
  stand until the next survival check pops one (brief 10.4).
- A stand that crosses a line between the two hits of a survival break needs two more
  (decision 6).
- TNT and hopper carts are inert until N6; containers do not open (decisions 2 and 3).
- What N4 named stands.

## 6. Open questions for the review

1. Does decision 7 (logout in a boat) fit ADR-0020's stays without a new ordering
   problem, or should it wait for N7, where animals ridden alone raise it again?
2. Is the runtime state of decision 9 better as one field of `Entity` or in each kind's
   data?
3. Effects on entities (decision 10) were planned by the record for N7: is bringing them
   to N5 sound, and does any N5 use need its answer back in the same tick (a mount does:
   it is a hand-over, so it waits a tick)?
