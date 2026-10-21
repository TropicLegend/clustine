# N7j part R: riding the camel, the nautilus and the happy ghast (the lead's)

The plan of part R of N7j ([the plan](n7j-plan.md), sections 3 item 10 and 4.2), from
[the research note](n7j-riding-research.md) (cited as "R §n") and N7h's riding
([its plan](n7h-riding-plan.md), "H §n"), which every rule here builds on: a ridden mob is
a living root that its controller's client moves, N5's vehicle move checks it, and a unit
crosses a line whole. Built after parts C, D and E are on `main`, since it calls the hooks
in their files and mends one of them.

## 1. What a player sees

- **A camel** (and a camel husk) takes a player on a click, saddled or not, two at most;
  saddled, the first steers it, sprinting faster; the jump bar dashes it, with its sound,
  and it is ready again 55 ticks later with the ready sound. The riders sit 0.5 ahead and
  0.7 behind, lower while it sits and moving with its sitting and standing. A sitting camel
  takes no steering and no dash. Sneaking puts a rider off at its side, the main arm's
  first.
- **A happy ghast** wearing a harness takes up to four on a click (not sneaking), the
  goggles coming down at the first; the first rider flies it, rising with jump; the
  goggles go up when the last gets off, and it stays still ten ticks after any rider gets
  off and while a player stands on its top, when nobody steers it. Getting off puts a
  rider on its top. Its riders are never kicked for flying.
- **A tame nautilus** takes one on a click; saddled, the rider steers it under water and,
  slower, out of it; the jump bar dashes it, ready again 40 ticks later.
- Each across a pinned line as a boat: the unit first, its riders after it (N5).

## 2. The contract: hooks in `MobType`

Every type rule is a hook with the default that is the pig's and horse's behaviour now, so
nothing of N7h changes:

1. `can_add_passenger(&self, mob, passengers: usize) -> bool`: default `passengers == 0`
   (one seat). Camel `<= 2`, ghast `< 4`, nautilus the default. `region/riding.rs`'s
   `can_add_passenger` asks it for a living vehicle.
2. `seat(&self, mob, index, count, game_time) -> Option<Vec3>`: the passenger's attachment
   point before the vehicle's yaw turns it; `None` takes the type's extracted points
   (H §3 item 1). The camel's is `camel::riding::seat` (pose, pose time from the world's
   game time, R §1.4). `passenger_point` takes the game time. Riders are placed every tick
   by `position_rider` already, so the camel's moving seats follow its animation.
   `positionRider`'s body yaw for the camel (the horse's too, which N7h left out, R §1.4):
   a living rider's body yaw set to the vehicle's; for a player it is their client's, so
   only mob riders change (none yet), named.
3. `steered_by` (exists): camel and husk saddled (`camel::riding::steered_by_rider`), ghast
   harnessed and not on its still timeout (`happy_ghast::riding::steered`), nautilus
   saddled (`nautilus::steers`). Asked every tick and at every move (H §3 item 2), so a
   ghast that becomes still hands itself to its AI at once.
4. `tick_ridden` (exists): camel `camel::riding::tick_ridden(…, forward = 0)` (the server
   has no keys, R §2.1: a sitting camel is not stood up by its rider on the server, as in
   the game; probe 1 checks what a watcher sees); ghast `ridden_yaw` then `setRot`
   (R §2.3); nautilus `ridden_rotation` (R §2.4). Each turns the vehicle a second step
   after the client's move, as the game does (R §2.3), kept.
5. `flying_vehicle(&self, mob) -> bool`: default false; an adult happy ghast true. The
   floating test of `check_move` and `tick_drivers` skips a flying root (SGPLI @782–787),
   and its accepted move gathers no fall (`checkFallDamage` empty, R §2.5): a second hook
   `gathers_fall(&self, mob) -> bool`, default true, the ghast false.
6. `on_passenger_added` / `on_passenger_removed(&self, mob, count_before/after, out)`:
   the ghast's goggles sounds and still timeout (`passenger_added`,
   `passenger_removed`, R §1.2), the nautilus's `clearHome` where a mount left it empty
   (R §1.3). Called by `mount_player` and every path that unseats a rider
   (`leave_vehicle`, `eject_passengers`, `unride_removed`, the logout's take).
7. `riding_jump(&self, mob, power, moment) -> Option<JumpStart>`: `canJump` and
   `handleStartJump` (R §3): `None` where it cannot (a sitting or unsaddled camel, an
   unsaddled nautilus, a ghast, a pig); else the sound to make with the entity's volume
   and the voice pitch drawn **twice from the mob's stream**, then the `entity_action`
   game event, and `DASH` set. The horse family's existing `riding_jump` becomes this
   hook's horse implementation, unchanged. **The camel's `handle_start_jump` begins its
   55-tick cooldown where none runs** (R §3: the game's `onSyncedDataUpdated` runs on the
   server), mended in `camel.rs`; the nautilus's already does (40).
8. `dismount_at`: the camel by the horse branch of `dismount_location` with its own width
   (`camel::riding::dismount_directions`), the ghast at `(x, top, z)`
   (`happy_ghast::riding::dismount_at`), the nautilus as the pig (R §5). The water rule
   is unchanged and already right for all five (R §5).
9. `can_be_collided_with(&self, record, mob, mover_kind) -> bool` for other movers:
   default by kind as now; the ghast's from its state (R §4). `physics/entities.rs` and
   `region/colliders.rs` ask it per record where a mover meets a mob. A player's move is
   their client's and needs nothing (R §4).

## 3. The clicks

`Did::Ride` for the three from their `interact`: the camel and husk with fewer than two
passengers and not a baby (`doPlayerRide`: the rider's yaw and pitch set to the camel's,
as the horse's `Did::Ride`), else `FAIL` where the game's is (R §1.1); the ghast
harnessed, adult, not sneaking, `SUCCESS` even when full (R §1.2); the nautilus tame, not
sneaking, not its food (R §1.3). The camel's saddle menu by sneaking or the inventory key
(`open_mount_menu` for the camel: anyone, ridden or not, R §1.1), the nautilus's by its own
rider test: both are N6's horse menu with the camel's one saddle slot and the nautilus's
saddle and armour; built here if the menu code takes them without change, else named for
a step of their own.

## 4. The happy ghast's staying still and solidity

The still timeout and the scan are part D's (`after_tick`). R adds: the report of a still
ghast flagged `Tracked::PRECISE` so the edge sends exact positions, and the position sync
packet as it begins to stay still (the edge's `entity_position_sync` for a report whose
`stays_still` turned on; R §4). Solidity per record (§2 item 9) makes mobs, items and
vehicles stop on a still ghast.

## 5. Across a line

N5's crossing already carries several players a unit (R §7): nothing new for a camel with
two or a ghast with four. Named, as now for boats: a click across a line does not mount
(R §7 d), the second rider's seat may be in the next region's land and is read through the
copies, and a camel's pose is read by game time on both sides. Tests at a pinned line: a
camel with two riders and a ghast with three crossing, each rider seated after; a
nautilus dashing across. The logout is the logout design's already (R §8): with two or
more players the leaver is put off and the unit stays; the ghast's `removePassenger` then
stops it for ten ticks (§2 item 6 covers the logout's take).

## 6. The leash of a driven mob

`driver_of` in `region/leashes.rs` (R §4) takes a driven mob as it takes a driven boat, so
a leashed ridden ghast is pulled by its driver's movement.

## 7. Tests

The lead's, in `region/tests/riding.rs`'s and `horse_riding.rs`'s manner, in a new
`region/tests/mount_riding.rs`: each hook's rule (seats, the camel's by pose at several
pose times, against the formula's numbers; two riders and the first steering; the second
taking over when the first gets off; the ghast's four, goggles, still for ten ticks after
a dismount, not driven while still, no floating kick after 100 ticks aloft; dashes with
their sounds and two draws, the camel's ready sound 55 ticks later, refused while sitting;
dismount places; solidity). Then tests from this plan and the note by someone who wrote
none of it (part S's manner), and comparisons with the official server
(`mobs_oracle.rs`): a camel mounted by two, steered and dashed, and probes 1 to 4 of the
note's section 11.

## 8. Open questions for the review

1. Whether the camel's server-side sitting pose while ridden (probe 1) should wait for the
   probe before R is built, or R copies the game's code as read and the probe confirms.
2. Whether `on_passenger_removed` is reached by every path that unseats a rider,
   including a rider's stay ending by a lapse of a pending seat and the logout's take.
3. Whether the seat hook should take the vehicle's record (for its pose and game time)
   rather than the mob alone.
4. Whether the menus (§3) belong here or in a step of their own.

## 9. The review (2026-10-21) and what the plan is now

An independent review found eight defects, six risks and three nits; the plan stands
amended by them, and where this section and the ones above differ, this one holds.

1. **One helper unseats** (findings 1, 2, 16). `seat_left(vehicle, rider, …)` is called
   wherever a seat actually leaves `riding.passengers`, and only there: in `leave_vehicle`,
   in the seat branches of `eject_passengers` and `unride_removed` (where a player's seat
   is dropped, so `dismount_player` later finds none and calls nothing), in
   `dismount_player`, `unseat_leaving`, the lapse of a pending seat
   (`count_pending_seats`), `drop_pending_seats` and `unit_returning` unseated. For a
   player it runs after the seat is gone and before the `entity_dismount` vibration
   (`removePassenger` before `gameEvent`, Entity 7051–7080). `seat_taken` likewise from
   `mount_player` and `start_riding`, with the ghast's goggles sound **before** the seat
   is added and before `entity_mount` (HappyGhast 539–568). **The logout's take** plays the
   goggles-up sound to watchers and leaves the taken records as they were (the game saves
   the unit before `stopRiding`); **the re-entry seating** (`unit_returning`) calls
   `seat_taken` as `startRiding(…, true, false)` does; a crossing's `rider_arrived` does
   not. A dead ghast's goggles-up at `unride_removed` is kept.
2. **The nautilus's `clearHome`** is the click's, after a refused mount (finding 3).
3. **The seat hook takes the vehicle's record** (finding 4, Q3): `seat(record, mob, index,
   count, passenger_kind, game_time) -> Option<Vec3>`, the point **already turned** by the
   vehicle's yaw, as `camel::riding::seat` returns it; the count includes pending seats.
   Both callers of `passenger_point` (`position_rider`, `seat_player`) use it; `None`
   keeps the type's points and the horse's rearing offset as now.
4. **The clicks** (finding 5): `Did::Ride` (the rider turned to the mob) for the camel and
   husk only; `Did::Mount` for the ghast and the nautilus. `Used` gains the passenger
   count, pending seats counted and the clicker's own stale seat not. The camel's `FAIL`
   is an `Interaction` variant `Fail` if `Pass` would let the item's use run where the
   game's would not; else `Pass` with the reason written.
5. **The menus are built here** (finding 6, Q4): the camel's (anyone, ridden or not,
   always tame, one saddle slot) and the nautilus's (its own rider test, saddle and body
   slots), by sneaking and by the inventory key, through `open_vehicle_inventory`,
   `mount_facts` and `write_mount_slots` gaining the two.
6. **The nautilus turns half way** (finding 7): `yaw = ridden_yaw(yaw, rider_yaw)`, then
   `setRot(yaw, rider_pitch · 0.5)`, head and body to the yaw; each type's tuple order is
   written in its code.
7. **The camel's cooldown starts where `DASH` changes and none runs** (finding 8): in
   `handle_start_jump` and at the dash's end in `after_tick`; a second command in the air
   sounds again but does not restart it. The ready sound's tick (T + 54 for a command at
   T) is pinned by a test and checked by probe 2.
8. **The ghast's added hook scans** (finding 9): the region scans its players and faces
   for one above the ghast by `scan_player_above`'s rule, so a mount with a player on top
   leaves the timeout at 10 and sends nothing twice.
9. **`JumpStart` is per type** (finding 10): the sound, its volume, whether the pitch is
   drawn (two draws) and whether `entity_action` follows. The horse's stays 0.4 and 1, no
   draw, no event, tested.
10. **`driver_of` is `Ridden::Driven`'s test** (finding 11), a change to N7h named and
    tested with the pig (with and without its stick).
11. **The ghast's ambient interval is six times as long while ridden** (finding 12): the
    hook gains `carries`, built here.
12. **A click across a line mounts** (finding 13): planned here, after the rest, as an
    `Effected::OnEntity` mount carried to the holder, which seats the clicker and hands
    them over as N5's crossing does; if its design shows a seam of its own it gets a note
    and a review before it is built, and until then the roadmap says exactly what to try
    and what fails.
13. **The edge** (finding 14): a still ghast's report sends `entity_position_sync` at
    once, before the tick's data update with `STAYS_STILL`, to its riders too (a new
    report flag); precision is lasting, so the report that ends the stillness clears it.
14. **Solidity carries the mover's kind** (finding 15): `Collider` keeps "solid to happy
    ghasts" beside `solid`.
15. **The floating kick** (finding 17): only `check.floating = false` for a flying root,
    as `handleMoveVehicle` alone exempts it; `tick_drivers` is left alone.
16. **Probe 1 runs before R's commit** (Q1), on a quiet machine: what a watcher sees of a
    sitting camel ridden forward decides the server's seat height for its riders and its
    `canJump`; R is built with `forward = 0` as read, and changed if the probe says.
