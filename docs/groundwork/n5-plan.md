# Step N5: vehicles, riding and what a creative player places, planned

Draft of 2026-10-18, revised the same day after an independent review (section 7,
which overrides what it amends above it); to be reviewed again before it is built. Step N5 of
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
  A boat nobody drives floats, drifts with currents, and rises and sinks in bubble
  columns (it picks up animals from N7 on). Players stand on boats; items
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
- A player who leaves while sitting in a boat or a cart gets off as they leave, the
  vehicle staying where it was; they come back beside it (section 7, item 1: the game
  takes a vehicle away with a lone rider, a named difference for now).

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
7. **Withdrawn by the review** (section 7, item 1): a leaving player is dismounted in
   the commit of their leave and the vehicle stays in its chunk, as the game does with
   two riders; the game's taking of a vehicle away with a lone rider is a named
   difference.
8. **Containers' contents in a record** are the stacks as N2 encodes them; a chest
   boat's 27 stacks fit one record (N2's large entity split covers them anyway).
9. **Amended by the review** (section 7, item 7): the state below is saved like any
   other field, in each kind's data, the common ones on `Entity`. Unsaved state a player
   would notice at a line travels with the entity (brief 10.5). A vehicle's damage, hurt time
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

## 7. The first review, and what it changed

An independent review (2026-10-18) found these; each change overrides the text above.

1. **Decision 7 lost a boat at a quick reconnect.** The leave's commit takes the unit out
   of its chunk unconditionally, while a stay note below the store's floor is dropped
   (ADR-0020, section 3; `lanes.rs`'s commit arm), so a leave racing the new stay's
   `Entering` left the unit in no durable state. ADR-0020 also writes no note at a leave.
   Withdrawn: a leaving player, and one who dies, ends or is reset by their edge, is
   dismounted in the commit that removes them, and the vehicle stays in its chunk. The
   game's lone-rider rule is a named difference, to be built with a note kind of its
   own and an amendment of ADR-0020 when animals that are ridden (N7) raise it again.
2. **A rider was shown dismounted at every crossing.** The unit reaches the new region
   before the player, and its first report would list only seated passengers, so the
   rider's own client would see the boat without them. So:
   - a pending player keeps their seat: their index in `passengers`, and every rule of
     the game but positioning (`canAddPassenger`, `isVehicle`, the controller rule, the
     cart's pick-up) counts them;
   - the edge shows a player as riding from the player's own region's word (their state
     says "rides K at index i"); a vehicle's report supplies only its non-player
     passengers. So a player is never drawn in two vehicles, whichever region speaks
     first;
   - reports of passengers obey the edge's `hops` rule, as moves do.
3. **Players are not in copies**, so nothing in one region sees another region's
   players: a leash held across a line cannot hold, and carts and boats pass through
   players over a line. Following ADR-0028 (section 5): a leash whose holder is in
   another region snaps, as one held beyond reach does, and where regions follow players
   a held leash at a line counts as entities working at it, so the two merge; a cart or
   a boat passes through a player beyond a pinned line. Named differences. Publishing
   players into copies is left for a later step.
4. **Kept `MoveVehicle`s were checked as one tick.** A hand-over takes one to three
   ticks, and every move sent meanwhile reaches the new region together. So: while the
   controlling seat is pending the new region keeps the boat still, as for a driven
   boat; at seating it seeds the move check from the root's position, as `tickPlayer`
   would; the kept moves are a sequence (at most 20, the older dropped), each checked
   as one client tick against the last; the hand-over carries only the floating counter
   and `vehiclePositionLastResetAt`.
5. **"Seated, vehicle pending" made precise.** A pending entry keeps its index (the
   `addPassenger` rule is for new mounts only). While pending, sneaking, interacting and
   mounting elsewhere are refused; an ejection drops the pending entry, and the player is
   dismounted when they come. A player who comes to a present unit that has dropped
   their seat is dismounted at once. A unit sent on to a third region while a seat is
   pending leaves a forwarding entry (key to chunk) for 100 ticks, so that the player
   follows. The deadline and the kept moves are in the player's saved state, not
   runtime state. A take-in from the store (a restore after a checkpoint, a fold) drops
   player passengers the region does not have.
6. **Phantom seats from the other removal paths.** `remove_player`, `end_player`,
   `apply_dead`, `apply_arrival`'s replacing branches and the edge's reset each take the
   player out of their vehicle's `passengers` in the same commit and report it.
7. **Runtime state left out of saves contradicted the checkpoint** (`messages.rs`: every
   change committed up to the checkpoint's tick is in a chunk saved before), and merges
   and splits reload warm chunks. There is no such category: the state of decision 9 is
   saved, and per-tick counters (the hanging entities' 101-tick check, decaying hurt
   times) follow N2's rule of counting, relative to their record, committed when they
   matter, not every tick.
8. **The contracts lacked:**
   - `PlayerInput::Keys { forward, backward, left, right, jump, sneak, sprint }` from
     `player_input` (the edge keeps sneaking alone today), kept in the player's state and
     carried by the transfer, since the client sends it only on change: a cart crossing
     a line must keep the rider's held key; `START_SPRINTING`/`STOP_SPRINTING` from
     `player_command` (nothing keeps the sprint flag yet);
   - the player's state: the vehicle's key and index, the move check's state, the
     pending deadline and kept moves, a mount intent;
   - arrivals addressed by the root's chunk for every member (an arrival is filed by
     `entity.chunk()` today);
   - a `RemoteStep` for an entity's item used on another region's block (a cart placed
     on a rail across the line);
   - the 160-tick floating kick;
   - `VehicleCorrected`, `Mounted` and `Dismounted` as `PlayerEvent`s, to one player.
9. **Uses that need an answer back.** A mount is not an effect: it is the player's
   hand-over to the unit's region with a mount intent, which runs `startRiding`'s checks
   there and hands the player back if refused. An armour stand's take and swap give the
   player an item: across a line they do nothing in N5 (named), hits on it work. Effects
   on entities in N5 are hits (a creative break), knots, item frames (put an item,
   turn it) and pick-item, done in the next tick's seams, a tick after a local use.
10. **Shared code no part owned.** `VehicleEntity`, `ContainerEntity`,
    `BlockAttachedEntity`, the comparator's reading of entities in `diodes.rs`, the
    dispatch of items used on entities and blocks, the remote-action path and the
    edge's entity data per kind go into N5b, which is mine, before anything is
    delegated. N5g waits for N5c as well as N5f; the cart's pick-up of boats comes in
    N5f, after N5e; the stand's push of carts in N5e, after N5d.
11. **Smaller points:**
    - a rider departs where the store says the chunk is another's (`region.rs`'s
      departures), not "where the region does not tick"; a crossing within a packet
      stops the old region applying that player's later inputs, and sets their last
      input at the crossing packet;
    - a click without sneaking on a chest boat seats the player (decision 3 keeps it);
    - the rider's push speeds a cart only under 0.1 a tick (brief 0.3);
    - decision 6 never arises on a creative-only server, where a hit breaks the stand;
    - a moving empty cart nudges a player (`LivingEntity.push`), which the player's
      client must be sent as a motion;
    - whether `paddle_boat` comes every tick or on change is checked with a real
      client; the region keeps the last it was told either way;
    - wire ids are never reused (N1), so the record's quarantine of ids is not needed;
    - `MoveVehicle` is held back by the edge's `awaiting_teleport`, as moves are;
    - the oracle's cart comparison reads the official server's cart by its position
      sent to a client from a known tick of placing, as the redstone comparisons read
      blocks.
