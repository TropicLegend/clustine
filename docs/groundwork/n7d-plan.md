# N7d: sleep, the bed and the respawn point

The rules are the game's, from `n7d-brief.md` (cited as B). The plan's decisions on sleep
are `n7-plan.md` 7.4.4, 8.4.4 and 9.3. This says how Clustine builds them.

## 1. The player's state

`PlayerState` (and the region's `Player`, the transfer and the store's place where said):

- `sleeping: Option<Sleep { head: BlockPos, since: u64 }>`: the bed's head (the game's
  `sleepingPos`), and the **game time** they lay down at (`WorldClock::game_time`), so
  that the sleep counter is `min(100, game_time - since)` without a commit a tick
  (B 3.1). The counter's run on from 100 to 110 after a wake is the client's fade, and is
  not kept (named).
- `respawn: Option<Respawn { pos: BlockPos, yaw: f32, pitch: f32 }>` (B 2.1; `forced` is
  always false for a bed and is not kept). In the state, the transfer and the place; the
  place's respawn raises the store's format to 12.
- A transfer carries `sleeping` too, though a sleeper is not handed over by walking: a
  split moves them with their state, and a hand-over can only happen if a merge or a
  reshape left them in a chunk the region believes another's.

The face's `sleeping` (and `WorldPlayer::sleeping`) becomes that game time, not a counter:
another region reads `min(100, its game time - since)` (named: up to a second's
difference in the clocks, n7-plan 9.3.2).

## 2. Using a bed (`PlayerInput::UseItemOn` on a bed block, any hand content)

As B 1.2 and 1.5, in that order, with the overworld's constant bed rules (B 1.3): a bed
sleeps when dark and always sets the spawn; a straw bed sleeps when dark, never sets the
spawn and breaks when its sleeper leaves. Dark is `sky_darken(clock.day) >= 4`, with the
f32 formula of B 1.4 for clear weather, in a new `time.rs` of the sim (the day timeline's
keyframes and `skyDarken`), evaluated at the input's tick from the clock as the last tick
left it (B 1.4: `skyDarken` is the last level tick's).

- The other half missing: nothing. The head is used for everything.
- Occupied: action bar `block.minecraft.bed.occupied` (no villager to kick until N10).
- `startSleepInBed`: asleep already or dead (never): nothing; too far (B 1.5 step 4):
  `too_far_away`; obstructed (the block above the head or above the foot suffocates, as
  `living.rs` decides suffocation): `obstructed`; the spawn set if the rule says (chat
  `block.minecraft.set_spawn` where the position changed); not dark: `no_sleep`; monsters
  near: none until N9, and every player is creative, who skips the check; then lie down.
- Lying down (B 2.2): the player put at the head's centre, `y + sleep height + 0.125`
  (0.6875 a bed, 0.375 a straw bed), motion zero, a passenger gets off, the head's
  `occupied` set (the foot follows by the existing `updateShape`), `sleeping` set; their
  client teleported (as `Rides` does), and the sleep status updated and announced (§4).
- The edge shows a sleeper to others by the synced pose (index 6, `SLEEPING` = 2, a new
  `EntityDataValue::Pose`) and sleeping position (14, `OPTIONAL_BLOCK_POS`), and their
  hitbox 0.2 × 0.2 for the region's collisions.

## 3. Asleep, and waking

- Movement inputs are not applied while asleep; one more than a block away is answered
  with a teleport back (B 8.2).
- At the end of each tick (the game's `Player.tick`, after the level's sleep check): the
  bed rule wakes (`stopSleepInBed(false, true)`) once it is not dark; a bed gone wakes at
  once (`stopSleepInBed(true, true)`).
- `stopSleepInBed(immediately, update)` (B 3.3): the wake animation (animate 0) to those who
  see the player and the player; `occupied` cleared on the head; a straw bed broken (its
  sound, the head to air); the stand-up position by `findStandUpPosition` with the existing
  dismount helpers (`block_floor_height`, `non_climbable`, the box's collisions), danger
  checks with the game's dangerous blocks as N5's dismount has them; yaw facing the bed,
  pitch 0; the client teleported; the sleep status updated if `update`.
- The leave-bed input: the edge passes `player_command` `STOP_SLEEPING` as a new
  `PlayerInput::LeaveBed`; the region answers `stopSleepInBed(false, true)`.
- A player who leaves the world asleep: their bed's `occupied` cleared (no update of the
  status, B 3.2 item 6).

## 4. The night, across regions

Each region, every tick, after its inputs and before its players' end-of-tick (B 4.3):

- The level's players are the world's players the store last published (`WorldPlayer`),
  with the region's own players in place of their entries and added where missing.
  Active: not a spectator (everyone, until survival). Sleeping: `sleeping` set. Deep:
  `game_time - since >= 100`.
- `players_sleeping_percentage` is 100 (no game rules yet): needed is
  `max(1, ceil(active * 100 / 100))`.
- Enough sleeping and enough deep: the region moves its clock's `day` to the next multiple
  of 24000 (B 4.5; unchanged at a multiple), wakes its own sleepers with
  `stopSleepInBed(false, false)`, and reports the clock jumped (the edge sends the full
  clock at once, as the game sends it to everyone). Other regions take the later clock at
  the next publication, and their sleepers are woken by the bed rule then (n7-plan 9.3.1:
  in a tick whose clock jumped, the bed rule's wake runs before the sleep check, so a
  jumped clock never skips a second night).
- The announcement (B 4.4): when the pair (sleeping, active) of the region's view changes
  and either side has sleepers, every player of the region is sent, on the action bar,
  `sleep.skipping_night` if enough sleep, else `sleep.players_sleeping` with the count and
  the needed. Named: other regions' sleepers count a second late, and a region announces to
  its own players only.

## 5. What else

- The respawn point is kept but not yet used: nobody dies until survival.
- Named: statistics, the advancement, phantoms, villagers in beds, explosions of beds in
  other dimensions; weather (always clear), so neither rain nor thunder changes the night.

## 6. Tests

From B by someone who did not write the code (in N7g), and by me with each part: `sky_darken`
against B 1.4's table (12523 and 23477 the first and last dark tick, the steps every 164
ticks); a bed's use by day sets the spawn and says `no_sleep`; at night the player lies
down at the bed's height, the bed occupied, the status announced; too far, obstructed,
occupied; the leave-bed input stands them up beside the bed facing it; a straw bed breaks
and never sets the spawn; one sleeper skips the night on the 101st tick, the clock at the
next multiple of 24000, everyone woken, no message after; two players at 100 % need both;
a sleeper in another region counts from the world list; a clock jumped to the morning wakes
another region's sleepers before its sleep check; and the edge's pose and sleeping
position, and the set_time at a skip.

## 7. After the review (all findings taken)

1. **A region skips only with a sleeper of its own**, deep and asleep, and only when it is
   dark by the last tick's clock: in the game sleepers exist only then. A region without
   sleepers takes the morning from the store's publication. Without this a region with no
   players could skip again every tick on a stale world list.
2. **The skip comes right after the inputs**, before block ticks and entities (which see
   the morning, as in the game), and sets the tick's output clock, `clock_jumped` and the
   delta's clock, so that the commit, the store's ratchet, a restore and the edge's
   set_time all have it.
3. **`since` is the region's own game time**, read with `saturating_sub`. The face carries
   it; the store publishes, in `WorldPlayer::sleeping`, the counter
   `min(100, game time of the writer's last commit − since)`, low by up to a second, the
   conservative side n7-plan 9.3.2 agreed. A region's own ratchet jump ages its sleepers by
   the jump (named).
4. **A taken clock that crosses a `wake_up_from_sleep` marker is the skip**, in every
   region: its sleepers are woken with `update = false`, the sleeping count zeroed, and
   nothing announced. The announced pair (sleeping, active) is memory, seeded silently from
   the first view after a restore, a merge or a split.
5. **The sleeper's own client** is sent its pose and sleeping position (the in-bed screen
   and its Leave Bed button read them), and both are cleared at the wake.
6. **Beds at a line**: a bed is slept in only by a player whose region holds the head's
   chunk; otherwise the use does nothing and says nothing (named). The stand-up search and
   `checkBedExists` read copies, and a block the region cannot read is unsafe.
7. **Every way a sleeper leaves wakes them first** (`stopSleepInBed(true, false)`): a leave,
   a replacement, a dead stay, a discard, an edge gone, a let-go. The place kept is the
   stand-up position, and a straw bed breaks.
8. **The ticks**: `since` is the game time after the increment of the tick that applies the
   input; "dark" at an input is the previous tick's clock; the end-of-tick bed-rule wake
   reads the tick's clock. A sleeper who lies down in tick T is deep in the sleep check of
   tick T + 100, the game's 101st server tick.
9. A sneaking player holding an item uses the item, not the bed (the existing click path
   decides). The skip's set_time follows the wake packets, where the game sends it first
   (named). A player who just joined elsewhere may be missing from the world list for up
   to a second, allowing a skip the game would refuse (named).
