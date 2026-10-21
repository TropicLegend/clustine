# Getting on a vehicle across a line (N7j part R, section 9 item 12)

A player standing in one region who clicks a vehicle that region shows only in a copy
(the vehicle's chunk is another region's land) gets on nothing today, for a boat, a cart,
a pig, a horse or the new mounts alike (`region/animals.rs`, "one across a line does not
get on, as on a boat"). With a happy ghast four blocks wide and two players to a camel,
a click across a pinned line is likely in ordinary play. This note designs getting on
there, from N5's crossing, which already hands a rider to the region of their vehicle.

## 1. What a player sees

They click the vehicle and are seated on it, as within a region, a few ticks later (the
time an effect takes there and back, and a hand-over). Nothing else they see differs: the
use's own effects (a saddle put on, a harness, food) already reach the holder as an
entity effect (`EntityUse::Interact`) and are done there.

## 2. The rule it rests on

A riding player belongs to the region that holds their unit's root chunk, their *holding
chunk* (`riders.rs`), wherever they stand; a crossing hands them on with their seat kept
for them (`Seat::pending`, `PENDING_FOR` ticks, dropped with their edge), and
`rider_arrived` seats them when they come. Getting on across a line is the same hand-over,
begun by the vehicle's holder granting a kept seat.

## 3. The steps

1. **The click (region A, the player's).** As now: the click on a copy's entity is an
   `EntityUse::Interact` to its holder (B). A records nothing.
2. **The use (region B, the vehicle's).** B applies the use as it does today. Where it
   ends in a mount (`Did::Mount`, `Did::Ride`, `Did::MountOrClearHome`, a boat's or a
   cart's `mount_player`) for a player who is not B's own:
   - B runs the vehicle's rules as for its own player: `can_add_passenger`, the boarding
     cooldown, the vehicle's own refusals (a boat out of control). A sneaking player was
     refused at A's click already (the use carries `sneaking`).
   - B calls `passenger_adding` (the ghast's goggles), then adds a **pending seat** for the
     player's entity, kept `PENDING_FOR` ticks, of the player's edge (the use gains the
     edge; see section 4), and reports the passengers.
   - B sends A the grant: `EntityUse::SeatGranted { vehicle, chunk, turn }` to the player's
     entity, at the player's block (from the face B has of them), where `turn` is the
     rotation `Did::Ride` gives (a camel's or a horse's head yaw and pitch), or none.
3. **The grant (region A).** Where the player is still A's, rides nothing and is no
   one's on their way: A sets their riding to the vehicle, pending, its holding chunk the
   vehicle's, and applies `turn`. At the end of the tick A hands them on to B, as at a
   crossing, with their riding. Otherwise A answers `EntityUse::SeatDeclined { player }`
   to the vehicle, and B drops the seat at once (`drop_pending_seat_of_player`'s rule).
4. **The arrival (region B).** `rider_arrived` seats them on their kept seat: the mount's
   teleport, `entity_mount`, `Rides`, as for one who crossed.

A seat granted and never taken lapses after `PENDING_FOR` ticks, as every kept seat
does, and its going calls `passenger_removed` (the ghast stops ten ticks), as a lapse
does now.

## 4. What changes

- `EntityUse::Interact` gains the player's `edge` (format: a new entity use number for the
  new form, the old one read as before with no edge, which grants no seat).
- `EntityUse::SeatGranted` and `EntityUse::SeatDeclined`, each with a format number.
- `Effected::OnEntity` aimed at a player's entity is taken by the region that has that
  player (no chunk record has a player's key): a grant or a decline only.
- B's mount paths take a player who is not its own through step 2; A's applying of a
  grant through step 3.
- Nothing in the edge: it is told `Rides` at the seating, as after a crossing.

## 5. Ordering hazards, each with its answer

1. **The player moves, sneaks, rides something else or leaves between the click and the
   grant.** A checks at the grant (step 3) and declines; B drops the seat. A player who
   left: no player, the seat lapses or goes with their edge.
2. **Two players take the last seat**, one of B's own and one across: B applies its own
   inputs and the effect in its tick's order; whichever comes first gets the seat, the
   other is refused as within a region.
3. **The vehicle moves on (to C) before the grant is taken.** The seat goes with it as a
   kept seat does; A hands the player to their holding chunk, B's, which forwards them as
   it forwards a rider whose unit went on (`forwarded`, `rider_arrived`).
4. **The vehicle is gone before the use arrives.** The effect finds nothing, as any
   effect on a gone entity; nothing is granted.
5. **A crash between the grant and the hand-over.** The seat is in B's record, durable;
   A's grant is an effect in B's commit, so it is replayed or lost with it. A replayed
   grant finds the player already riding (declined, harmless) or not (granted again). A
   lost one lets the seat lapse. No player is ever seated twice, as only `rider_arrived`
   seats, and only on a seat that names them.
6. **The player's region is reshaped meanwhile** (a merge of A and B): the grant is
   taken by whichever region has the player, as every effect on an entity is.

## 6. Tests

Region tests at a pinned line (the pig's specification tests' manner): a click across the
line on a camel, a ghast, a boat and a pig seats the player after the hand-over; a full
mount refuses; a player who sneaks or walks off before the grant is not seated and the
seat goes; a vehicle that moves on is ridden where it went; a lapse of a granted seat
stops a ghast for ten ticks. End-to-end (`bin/clustine/tests/riders.rs`): a bot on one
side of a pinned line clicks a boat on the other and drives it.

## 7. Open questions for the review

1. Whether the grant should go to the player's entity by its face's position, or by a
   region-to-region message that names the player (none exists today).
2. Whether A should hold the click (mark the player "asking") so that a second click
   before the grant does not ask twice.
3. Whether the few ticks' wait is acceptable to a player, or a mount across a line should
   instead merge the two regions as an explosion at a line does (N7n).
