# N7h: riding a mob (the lead's part)

The plan of the riding of N7h (`n7-plan.md`, section 11: "N7h tamed and ridden, pig
riding, the logout on a mount (riding mine)"), from the game's rules as
[the research note](n7h-riding-research.md) read them in the 26.3 dumps (cited below as
R1 to R9, its sections) and from N5's vehicles (`n5-brief.md`, `n5-plan.md`). First the
pig, which every rule of a ridden mob shows; then the horse, once the horses are built
(their part is delegated, and exposes what the riding needs); last a player who logs out
on a mount.

## 1. What a player sees

- A saddled pig, clicked with anything but its food and not sneaking, takes the player on;
  the player sits at its back (feet 0.86875 − 0.6 above its feet, R6).
- Holding a carrot on a stick in either hand, the player steers it: it goes where they
  look, its body and head turned to their yaw and half their pitch; a use of the stick
  boosts it for 140 to 980 ticks (R4), shown to every client by its data; a creative
  player's stick takes no wear. Without the stick the pig wanders on with the player on
  it, as the game's does (R2, R3).
- Sneaking puts them off beside it, at the first free place to its right, left, behind…
  (R5); a pig that dies or is removed puts them off; a rider whose eyes go under water is
  put off (the pig is in `#dismounts_underwater`, R5).
- A ridden pig going off a ledge hurts itself by its fall and its rider by the same fall
  (R3; Clustine's players are creative, so the rider's is nothing, but the order is the
  game's).
- Others see the rider seated and the pig moving as the rider drives it; across a pinned
  line the pig and its rider cross together, as a boat and its rider do (N5).
- A player who leaves while riding a pig alone takes the pig with them, and finds it under
  them when they come back (R8).
- Horses, donkeys, mules and llamas likewise once built (section 7), with their own rules:
  no stick, jumps by the jump bar, rearing, the inventory key, the rider's body turned with
  the horse's.

## 2. What of N5 is reused unchanged

The seats and their order (`riding.rs`: `start_riding`, `add_passenger`,
`leave_vehicle`, `eject_passengers`, `unride_removed`, `tick_passengers`,
`position_rider`), the mount and dismount teleports and the riding player's double tick
(`riders.rs`), the sneak dismount, the dismount helpers (`offsets_for_direction`,
`is_floor_valid`, `block_floor_height`, `dismount_poses`, `escape_vector`), the vehicle
move's checks (`physics::vehicle_move`: too quickly, moved wrongly, refusal, the floating
kick), the edge's showing of passengers (`set_passengers` by the riders' own word,
d879077/b1f7998) and the crossing of a unit with a player in it (`riders.rs`, the unit
handed on whole, N5's section 8). A pig is an ordinary root vehicle to all of it (R,
"What of N5 applies unchanged"). What changes is below.

## 3. The contract: a mob as a vehicle

1. **Seats.** `can_add_passenger` for a mob: no passenger yet (`Entity.canAddPassenger`,
   one rider) and the type's own condition (the pig: none beyond its mounting click). The
   seat is `passenger_point` from the type's data, which the generated
   `EntityTypeInfo.passenger_points` has (R6); `position_rider` with no rotation of the
   rider (the horse's in section 7).
2. **Who drives**: a `MobType` hook `controller(&self, mob, first: Option<RiderSeen>) ->
   bool`, where `RiderSeen` is the first passenger if it is a player of the region, with
   both hands' items. The pig: saddled, and the player holds a carrot on a stick in either
   hand (R2). The region asks it **every tick and at every move**, so a change of the held
   item hands the pig back to its AI at once (R2). `boats.rs`' `drives` becomes "a boat's
   first player, or a mob whose type says so". A rider is always of the region that holds
   the root (N5's unit), so the hands are the region's to read.
3. **The server's tick of a driven mob** (R3), in the mob tick's skeleton (`mobs/tick.rs`,
   the order of `LivingEntity.aiStep`): `motion *= 0.98`; no `serverAiStep` (goals,
   sensing, navigation, controls); then `travelRidden`: the type's `tick_ridden` hook (the
   pig: rotation from the rider, `yRot`, `xRot · 0.5` each `% 360`, body and head yaw to
   it, the boost's tick), then motion zero. Nothing of the mob's own travel runs; the move
   is the client's (item 4).
4. **The move** (`boats.rs` `move_vehicle` and `check_root_move`): a `Root` made from the
   mob's living body as well as a boat's: its gravity (0.08), its box, its step height
   raised to `max(step, 1.0)` while driven (`LivingEntity.maxUpStep`), its bounce and fall
   as a living body's; the climbable reset of the fall distance before the move; an
   accepted move's fall check (`doCheckFallDamage`) with the mob's fall damage, which
   passes the fall to its rider first (`propagateFallToPassengers`, R3) — the rider's
   creative player takes none. The accepted move commits the mob's record (position,
   rotation, on ground, fall distance) as a boat's root is committed, with the rotation
   the next tick then overwrites with the rider's (R3).
5. **The boost** (R4): the pig's type state keeps `boosting` and `boost_time` (the
   game's, neither synced nor saved in the game; kept in the record here, as the record is
   durable every tick and a crash must not lose half a boost — named) and the synced
   `DATA_BOOST_TIME` (index 18, int). `boost(draw)`: refused while boosting, else
   `boosting`, `boost_time = 0`, the total `nextInt(841) + 140` from the pig's own stream.
   `tick_boost` in `tick_ridden` only. The edge shows index 18 by `shown` and its change
   by the tracker's dirty data. The carrot on a stick's use (`FoodOnAStickItem.use`) is a
   player input the region already has (`UseItem`): when the player drives a pig, the pig
   boosts; durability is spent by none of Clustine's creative players (R4).
6. **Getting off** (R5): `Animal.getDismountLocationForPassenger` by N5's helpers (the
   pig's motion direction's offsets, its own block height, the rider's poses), falling back
   to on top of the pig; N5's water rule with the rider's eyes and `#dismounts_underwater`,
   skipped in a bubble column.

## 4. What the edge does

Nothing new for the pig: the rider's client sends `move_vehicle` while it drives (the edge
passes it on as for boats), the passengers are shown by N5's rules, the pig's moves and
head by its tracker (its rotation the rider's), its saddle as equipment and its boost as
data. The tracking range of a ridden mob is its rider's (32 chunks, R7), which the edge
already applies to vehicles if N5 did so generally; check and name.

## 5. Across a line

The pig and its rider are a unit with a player seat: N5's crossing hands the unit on
whole, the vehicle first, the riders after it (`riders.rs`), and holds a driven unit back
at a line until the riders may follow (`relocate`'s `has_player_seat` branch). Nothing in
that is a boat's but `drives` and the root's tick, which section 3 generalises. Tests at a
line: a driven pig crossing a pinned line, the rider following; a pig that wanders over
with its rider (no stick); the same with the stick taken away in the middle.

## 6. Logging out on a pig (7.7.4; R8)

When a player whose root vehicle is a living mob with exactly one player passenger (them)
leaves, the region writes the mob into the player's place with them
(`LoggedPlace.vehicle: Option<Entity>`, the store's format raised) and removes it from the
world with them (`Gone`, as the game's `UNLOADED_WITH_PLAYER`); at their next entry the
entering region makes the mob again at the place with the player seated (a `Whole` of the
mob, the player's riding set, the mount's packets as `startRiding(…, true, false)`). With
the stay rules of ADR-0020: a place is written by the region the player left from; the
mob is in the place and not in any chunk, so a crash between the two cannot duplicate it
(the place and the `Gone` are in one commit, a group the store makes durable whole). A
vehicle that is no mob (a boat) stays in the world, as now. The ender pearls of R8 are not
Clustine's yet.

## 7. Horses (after the horse part)

The horse part exposes saddled, tamed, the jump strength, the speed, the passenger seats
and the dismount inputs. The riding then adds (R9): mounting by `doPlayerRide` (the
player's rotation set to the horse's), control without a stick, the inventory key
(`openCustomInventoryScreen`), jumps by `player_command START_RIDING_JUMP` (rear, sound;
the jump itself is the client's), the rider's body yaw set to the horse's, the rearing
offset in the seat, the horse's dismount (the side of the rider's main arm first), the
fall rule (no damage to the horse, none to the rider), and the llama's caravan with a
rider. Planned in a section of its own when the horse part is in.

## 8. Tests

The lead's tests of each rule above in `region/tests/riding.rs`'s manner; tests written
from this plan and the research note by someone who did not write the code; a comparison
with the official server (`bin/clustine/tests/mobs_oracle.rs`): a pig saddled, mounted,
steered with the stick and boosted, the packets a watcher is sent (`set_passengers`, the
pig's moves and rotation, index 18), and a dismount by sneaking.

## 9. Open questions for the review

1. Whether `boosting`/`boost_time` should be durable (section 3 item 5) or kept in memory
   as the game keeps them (lost at a crash, and at a hand-over unless the transfer carries
   them).
2. Whether the logout's mount (section 6) needs the store's format raised now, or can wait
   for the horses, and whether the place is the right home for it rather than a parked
   entity in a chunk.
3. Whether re-judging the controller every tick (section 3 item 2) can disagree with the
   client within a tick (the client changes its held item and sends a vehicle move in the
   same packet batch).

## 10. The review (2026-10-21) and what the plan is now

An independent review found eight defects; the plan stands amended by them:

1. **Logging out on a mount is taken out of this step** (findings 1, 2, 3, 7). As written
   it would lose the mob where a quick reconnect's `Entering` reaches the store before the
   leave's commit (the race N5's review found, `n5-plan.md` section 7 item 1), and on every
   removal of a dead, replaced or reset stay; the game takes any root vehicle with a lone
   player passenger, boats and carts too (`PlayerList.remove`, `saveParentVehicle`);
   re-entry needs the transfer to carry the unit where the place is another region's, a
   seated place, keys and hops for the remade unit, and the edge to take a riding word
   before `Present`; and `LoggedPlace` is a `Copy` record of the store's. It gets a
   design of its own, for every root vehicle, with the unit parked durable in its own
   chunk and the place holding a reference, an amendment of ADR-0020 and a review, before
   it is built. Until then a player who leaves on a pig is put off beside it, as from a
   boat now: named.
2. **A pig whose first seat is a pending player's is kept still** (finding 4): no AI
   step and no travel, as a boat's `driven` keeps it, so that the client's kept moves are
   judged from where it stood. `driven` is generalised with `drives`.
3. **The vehicle move's boat-only parts are changed, not reused** (finding 5):
   `check_root_move`'s root from a living body as well as `boat_tick_of`'s; the fall
   branch of `check_move` by the root's kind (a living root's `causeFallDamage` with
   `propagateFallToPassengers`, taken before the fall distance is cleared, a fall that
   kills it handled from the input phase); `tick_drivers`' gravity by the root's (80
   floating ticks for a pig).
4. **The pig's type state is versioned** (finding 6): its boost fields come in a new
   version read from the old layout, so no stored pig loses its variant; `shown` sends the
   kept boost total at index 18, non-default once set.
5. **The boost is durable** (open question 1): a crossing carries the record, and a boost
   held in memory would be forgotten at every line and drawn again by a second click.
6. **A dying pig keeps its rider until it is removed** (finding 8), the rider's moves
   accepted as the game accepts them while the rider holds the stick; the boost's draw is
   counted (`entity_drew`); a mount across a line (`Effected::OnEntity`) is tested; the
   tracking range of a ridden pig is tested.
7. **A kept move remembers whether its player controlled the vehicle when it came**
   (open question 3), and is judged by that at seating; the controller is judged from the
   region's own players only, never from faces.
