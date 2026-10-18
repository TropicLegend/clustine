# Step N6: TNT, explosions, containers that open, and the redstone that waits for entities, planned

Draft of 2026-10-18, revised the same day after independent reviews (sections 6, 7 and
8, each overriding what it amends above it and the sections before it); a third review of section 7 is answered in section 8, and a fourth review of section 8 in
section 9, which withdraws the lend at the moment of use; a fifth review of section 9 is
answered in section 10. Step N6 of the
[phase plan](n-plan.md): "TNT and explosions, and the redstone of ADR-0026 section 11
that waits for entities (dispensers, droppers, hoppers, detector rails, pressure plates
that count entities, tripwire)". The game's rules are in the [porting brief](n6-brief.md),
read from the 26.3 jar; this plan cites it by section (brief 2.4, brief 0.17) and
decides what it leaves open. Detector rails came with N5.

## 1. What a player meets after N6

- A chest, a double chest, a barrel, a shulker box, an ender chest, a dispenser, a
  dropper and a hopper open when clicked, with the game's menus: every click kind, drag
  and quick move, the creative player's picking, lids that open and close for everyone
  with their sounds; a chest cart, a chest boat and a hopper cart open too. A comparator
  reads what a container holds. Breaking a container spills what it holds.
- TNT is primed by redstone, a flint and steel, fire and another explosion, falls,
  flashes and explodes as the game's: blocks broken by its rays, their drops (TNT drops
  everything), entities hurt and pushed, players knocked back, chained TNT with its
  short fuse. A TNT minecart explodes on an activator rail, a hard landing or a hit.
- A dispenser fires what it holds on a rising edge: items dropped, buckets emptied and
  filled, bone meal, shears, flint and steel, TNT primed, boats and minecarts placed,
  armour put on whoever stands in front, shulker boxes placed. A dropper drops, or puts
  into the container it faces.
- A hopper pulls from the container above and the items on top, and pushes into the
  container it faces, locked by redstone; a hopper cart pulls as it rolls.
- Wooden plates are pressed by items and anything else, stone plates by living things,
  weighted plates by how many; tripwire is tripped by any entity.
- Sculk sensors and calibrated sensors hear steps, landings, blocks placed and broken,
  doors, levers and the rest, and give their signals.

## 2. Decisions the brief leaves open (taken here; the owner can overrule)

1. **Containers open in N6** (brief 0.1): the menus come with this step, as nothing
   else lets a creative player fill a dispenser, a dropper or a hopper. The crafting
   grid of the player's own inventory stays as it is (crafting is a later step, with
   the crafter, brief 0.16).
2. **The split of a menu** (brief 0.2): the region keeps the menu (which container, the
   carried stack) and runs `AbstractContainerMenu.clicked` on what it holds; it reports
   the menu's slots and the carried stack after each change; the edge keeps the
   container id, the state id and the client's remote slots and hashes, compares, and
   sends `container_set_slot`, `container_set_content`, `set_cursor_item` and
   `container_set_data` as `ContainerSynchronizer` would. The edge decodes the click's
   hashes and gives the region the click alone (slot, button, kind).
3. **No item components in N6** (brief 0.5): stacks stay item and count. A shulker box
   placed keeps its contents as a block; broken or dispensed it drops an empty box (a
   named difference, until the step that brings components); a client's hash of a stack
   with components never matches and the slot is sent again, as the game would.
4. **No container loot tables in N6** (brief 0.6): the structures' chests and barrels are
   empty as they are today (named; they come with the loot of structures, beside S3).
5. **The ender chest** opens the player's own 27 slots, which the player's state, their
   transfer and their place in the store (ADR-0020) carry from N6 on.
6. **Explosion resistance** comes from the extract program (`getExplosionResistance()`
   per block, brief 0.4), with the brief's table as its check; fluids' resistance read
   with it.
7. **Projectiles are N9's** (brief 0.12): a dispenser holding an arrow, a fire charge, a
   snowball, an egg or any of the 13 projectile items drops it as an item until N9
   (named).
8. **The crafter waits** for crafting (brief 0.16), as do the composter's recipes beyond
   its level (it composts and gives bone meal, brief 0.13).
9. **Game events and sculk sensors are in N6, last** (brief 0.3, section 8): the posts
   from every place the game makes one, and the sensors. If the review judges the posts
   too wide for N6, they become a step of their own after it and sculk is named.
10. **Across a line** (brief 0.17 to 0.23):
    - an explosion is computed whole in the TNT's region; its blocks across the line are
      effects that carry the call ("broken by an explosion of this radius, interaction,
      source and owner"), done by the holder a tick late, which evaluates their loot,
      spills containers, primes TNT and lays the drops, not merged with the near side's
      (named); entities across the line are hit by entity effects (the damage, the
      push); players across the line are not knocked back (players are not in copies,
      N5 plan section 7 item 3; named); the explosion packet goes to every player within
      64 blocks through the edges, each with their own knockback or none;
    - where regions follow players, an explosion at a line counts as work at it, and the
      regions merge;
    - a menu opens only where the player's region holds every chunk of its container
      (both halves of a double chest); at a pinned line one that does not is not opened
      (named); an opener of another region is not counted by the 5-tick recheck (named,
      rare);
    - a dispenser, dropper or hopper that would place or insert across a line waits, as
      a piston does (ADR-0026, section 10); named;
    - an entity across a line on a plate or a tripwire, or a vibration across one, is an
      effect to the holder of the block (ADR-0028, section 7), a tick late.
11. **Hoppers, plates, tripwire and sensors in the entity phase** (brief 0.13): the item's
    walk hands them back to the region, as a cart's `entityInside` does (N5).

## 3. Parts, in order

Each part ends in a verified commit on `main`. The contracts (N6a) are mine and pushed
before anything is delegated; the menus' edge part is mine (it is the edge's protocol and
a player's own state); the explosion across a line (N6g) is mine with the store's side.
The rest is delegated, each part with files of its own, and N6j's tests are written from
this plan and the brief by someone who built none of it.

- **N6a, the contracts.** `PlayerInput::ContainerClick { slot, button, kind }`,
  `PlayerInput::CloseContainer`; `PlayerEvent::MenuOpened { kind, title, slots }`,
  `PlayerEvent::MenuChanged { slots, carried, data }`, `PlayerEvent::MenuClosed`; the
  player's ender chest slots in their state, transfer and place (the store's player file
  format raised); an explosion's effect (`Effected::Exploded { … }`) and the entity
  effect of an explosion's hit; `RegionEvent::Explosion { … }` for the edges to fan out;
  `RegionEvent::GameEvent` only if the edge needs it (it does not: game events stay in
  the region); block resistance in the data crate (I run `cargo datagen`).
- **N6b, containers and menus** (the sim's part delegated, the edge's mine): the menus of
  brief section 3 for chests, double chests, barrels, shulker boxes, ender chests,
  dispensers, droppers, hoppers, chest carts, chest boats and hopper carts; openers,
  lids, block events and sounds; spilling; comparators reading containers.
- **N6c, TNT and explosions** (delegated): brief sections 1 and 2; the TNT cart.
- **N6d, dispensers and droppers** (delegated, after N6b): brief section 4.
- **N6e, hoppers, plates and tripwire** (delegated, after N6b): brief sections 5 and 6;
  the hopper cart's `Enabled` flag in its record.
- **N6f, game events and sculk** (delegated, last): brief section 8 and the posts of
  brief 0.3.
- **N6g, across lines** (mine): decision 10, the explosion's effects and their order in
  the store as phase E's are; the edges' fan-out of the explosion packet.
- **N6h, measured**: a crater of 64 TNT on the storage with its cache: the commit, the
  log and the tick's length; the oracle comparison of a TNT whose draws are fixed by
  where it stands (the rays' and the shuffle's draws differ, so by distribution: the
  set of blocks broken within a radius where none is ever spared, n-plan section 5).
- **N6j, tests from the specification.**

## 4. Named differences expected

- Shulker boxes broken or dispensed drop empty; structures' containers are empty
  (decisions 3 and 4).
- An explosion's drops at a line are not merged across it; its far blocks break a tick
  late; players across a line are not knocked back (decision 10).
- A menu across a pinned line does not open; a dispenser, dropper or hopper facing
  across one waits (decision 10).
- Projectiles dispensed drop as items until N9; the crafter waits for crafting.

## 5. Open questions for the review

1. Is the menu's split (decision 2) sound against the edge's hand-over: a menu open while
   the player is handed to another region (they cross a line with a chest open)?
2. Should game events and sculk (decision 9) be a step of their own?
3. Does the explosion's effect (decision 10) carry enough for the holder to do the game's
   order (updates at once per block, spills, chained TNT, then drops)?

## 6. The first review, and what it changed

An independent review (2026-10-18) found these; each change overrides the text above.

1. **An open menu across a restore, a merge, a split or a move.** The menu goes into the
   player's state (the container's place or entity key, its kind, the carried stack,
   the drag in progress and a serial), and the state's format is raised. At the first
   tick after any restore the region reports every player's menu whole, or that they
   have none; what it last reported is kept out of the state, so a restore reports
   everything and the edge's comparison heals the client. Openers' counts are worked out
   again from the restored menus, and the block event sent again. At the edge a
   `MenuClosed` is idempotent, and a `MenuOpened` while one is open is the game's
   `openMenu`: `container_close`, then the new id.
2. **A menu at a hand-over.** The region that lets a player go closes their menu in that
   tick, before the transfer is built: `closeContainer` puts the carried stack into the
   inventory, which travels in the transfer, `stopOpen` runs, and `MenuClosed` is
   reported, with the slots the stack filled; a tick's player events reach the edge
   before its outbox's `Departed`. A menu also closes when its container vehicle crosses
   a line, when `stillValid` finds a chunk of the container not held (splits, lends, a
   double chest's half), and when the player leaves (the carried stack put back, the
   place written). A remote click on a container and a use of a container vehicle as an
   effect consume the click and open nothing. Named: the menu closes at a line, not at
   8.5 blocks. So no menu spans two regions, and the opener counted by another region
   (decision 10) never arises.
3. **The edge lines up a click with the region's answer.** Menu events carry the menu's
   serial and the number of the last input applied. The edge keeps each forwarded
   click's hashes and its stale flag by input number and applies them to the remote
   slots (the game's `setRemoteSlotUnsafe`, `setRemoteCarried`) when an answer covering
   that number arrives, then compares, as brief 3.17 orders it; it drops events of
   another serial than the current one, and sends `container_close` only for the
   current menu. A region with no menu passes a click over.
4. **Dispensers, droppers and hoppers at a pinned line borrow, not wait.** They join
   ADR-0027's circuit parts (their reach: the block in front, the block below it, a
   double chest's other half), and are reported for the merging of regions that follow
   players as circuits are. "Waits" is kept only for what is out of reach still, and a
   waiting dispenser fails (its click) rather than firing minutes later. A drop or a
   primed TNT made in a foreign block is made in the dispenser's region and crosses as
   an arrival.
5. **Shulker boxes keep their contents** until stacks have components: one broken, by a
   player, an explosion or a piston (it is popped), spills its contents as a chest does
   (named), in every path of its removal.
6. **Plates and tripwire at a line**: their recounts and rechecks count the holder's
   entities and its copies' entities, so an entity across the line keeps a plate
   pressed without the 20-tick recheck letting it go; ADR-0026 section 10's "the region's
   own players" is amended so.
7. **N6a's contracts in full**: the effects (the explosion's block, its hit on an entity
   as an `EntityUse`, an entity come in for plates and tripwire, a vibration, a step on a
   sensor, TNT burnt by fire across a line); `EntityData::PrimedTnt` (fuse, owner, block,
   power), the TNT cart's fuse and the hopper cart's `enabled` (the minecart record's
   layout, here and not in N6e); typed container block entities; the item walk's
   hand-back for hoppers, plates, tripwire and sensors; the menu and the ender chest in
   the player's state; `PlayerInput` variants at the end; `PlayerInput::OpenVehicleInventory`
   for `player_command`'s `OPEN_INVENTORY` (a chest boat's inventory key). Worlds made
   before are refused and made again, as at N5.
8. **The ender chest in the store**: a new commit kind for stays with an ender chest, the
   old kinds read with an empty one, the players' file versioned; a click that changes
   its slots counts as a change of the player, so the tick writes a `Has` note; it is in
   the transfer. ADR-0020 section 2 is amended.
9. **The explosion's effect** is one per far block (`Effect` stays `Copy`): the expected
   state; the interaction (destroy, or destroy with decay, and the radius); whether a
   direct source exists; the owner's UUID and whether a player; an explosion id. The
   holder breaks each block in turn with its updates, spills, primes TNT, and after all of
   one explosion's blocks taken in that tick lays the drops, merged within that
   explosion's id only; effects of one explosion split across ticks lay apart (named).
   TNT burnt by fire across a line carries "burnt", and the holder primes it after the
   set. Where regions follow players, a TNT primed within 8 blocks of a line counts as
   work there, so that the regions merge within its 80 ticks and the explosion is whole.
10. **The order of the parts**: a, b, c, d, e in sequence, f last, g's edge fan-out of the
    explosion packet with c; or, where N6a has fixed the typed block entities, the
    tickers' slots and the item walk's hook, d and e side by side.
11. **Comparisons with the official server** for the new packets: open a chest, click,
    quick move, drag, close; the explosion packet's fields (N6h, N6j).
12. **Smaller points**: the edges are not sent a container's data on every hopper move (it
    is never shown); an ender chest opens across a line too (it touches the player's data
    alone); N6h measures on the disk without its cache as well, saying which number is
    which; a TNT cart is primed by a hit only while it moves at speed² ≥ 0.01, or by fire
    or an explosion; the plan amends ADR-0020 section 2, ADR-0026 section 10, ADR-0027
    section 2 and ADR-0028 section 7.

**For the owner** (decisions 4 and 7, which the review says a player meets at once):
structures' chests and barrels show empty when opened (no loot tables are stored, so
nothing is lost by it, and a later step fills chests not yet opened); arrows and other
projectiles in a dispenser drop as items until N9. Both are proposed as they are.

## 7. The second review, and what it changed

A second independent review (2026-10-18) read the plan with section 6 applied, the
brief, ADR-0020 and ADR-0025 to ADR-0028, and the code they name. Each change below
overrides what it amends above it.

1. **The edge keeps the menu's real slots, and every click is answered** (H1). The edge
   keeps a copy of the open menu's slots and carried stack, from `MenuOpened` and then the
   changes, beside the client's remote slots. The region answers every click it takes in
   a tick: one that changes something with the changes, one that changes nothing, is
   refused or is passed over with an empty change set; every answer carries the menu's
   serial and the number of the last input applied. The edge applies a click's kept
   hashes only to an answer of the click's own serial, and then compares every slot those
   hashes touched and the carried stack with its copy, and sends what differs, as
   `broadcastChanges` (or `broadcastFullState` for a stale click) does after every click.
   Kept hashes are dropped at a `MenuClosed` and at a `MenuOpened` of another serial. So
   two players at one chest, one taking a stack a tick before the other clicks it, leave
   no ghost stack on the second one's cursor.
2. **The serial is the number of the input that opened the menu** (H2). The `UseItemOn`,
   `Interact` or `OpenVehicleInventory` that opened it gives its input number, which is
   unique within a connection and comes out the same when the edge replays its kept
   inputs after a restore; no counter is kept in the state or the transfer.
   `ContainerClick` and `CloseContainer` carry the serial the client's screen is of, and
   the region refuses one that does not match its menu (the game's `containerId` test),
   answering with its menu whole, or with none. The edge takes a menu event as true when
   its last input applied is at least the edge's current serial, and drops it otherwise; a
   whole report of the current serial is sent as `container_set_content`, not as a close
   and an open. So a restore that reports an older menu while the client shows a newer
   one, whose opening is replayed and fails, cannot run the client's clicks on the older
   container.
3. **A menu whose container is held and not loaded yet stays open** (H3). After a
   restore, a merge or a split nothing is loaded at first. A menu whose container's
   chunks are held and not all loaded is neither checked by `stillValid` nor closed; its
   clicks are refused and answered with no slots; and in the tick its last chunk loads,
   the region reports the menu whole (which the edge sends as `container_set_content`,
   healing whatever the client guessed meanwhile) and counts its openers again. Only a
   chunk not held closes a menu. This replaces section 6 item 1's "at the first tick
   after any restore": a menu is reported whole at the first tick for a vehicle's
   container (the vehicle's chunk is loaded with it) and for one that has none, and at its
   chunk's load for a block's. The differential test of restores (`tests/stays.rs`,
   which compares a restored region's outputs with one that ran on, except the first
   tick's notes) also excepts these reports.
4. **A container across a pinned line is lent at use time** (H4; for the owner, below).
   Section 6 item 2 and decision 10 left a chest up to about five blocks across a pinned
   line unopenable, while doors and levers there work, and section 6 item 4 made it
   worse: a hopper borrowing the chunk of the chest in front of it would leave the
   lender's own players unable to open that chest. A player's use of a container whose
   chunks their region does not all hold asks for a lend of the chunks it lacks, as a
   circuit's part asks at build time (ADR-0027, section 2), and the use is kept in the
   player's state until the lend is granted, when the menu opens, or refused or 40 ticks
   pass, when nothing opens (named). A lent chunk with a container that a player of the
   borrower has open is kept (`must_keep_for_now`), as is one with a shulker box whose lid
   moves (item 5). Where the lender's own player then uses that container, the same rule
   asks for the chunk back the other way, which ADR-0027's one way per stretch refuses
   while the lend stands: they cannot open it until it is given back (named). ADR-0027
   section 2 is amended with a fourth asker, a use of a container.
5. **The openers' count is saved** (M1). The typed container block entity holds its
   openers' count, beyond what the game saves, so that a region restored, split or lent
   to knows what it last reported. Chests, trapped chests, barrels and ender chests heal
   through their 5-tick recheck, which compares the count found with the one saved and
   runs the close, the sound and the neighbour updates (a trapped chest's signal goes);
   a shulker box, which has no recheck, is corrected so when its chunk is loaded. A
   shulker box whose lid is moving is on the lender's wait list, as a moving piston is.
6. **The hand-over close comes in the player's tick** (M2). The region works out which of
   its players depart before the block-event phase and `keep_events`, and closes their
   menus in their player tick as the game's `stillValid` close is: the block events of
   `stopOpen` are kept with their chunk, a barrel's `open=false` is set within a phase,
   and what does not fit back into the inventory is dropped as an arrival at the player's
   new region, not laid in a chunk that is not this region's. This is ordering at the
   hand-over: N6b's hand-over and restore parts are mine, not delegated.
7. **The edge closes a menu at every `Departed`, and resyncs after a resume** (M3).
   Player events are not sent again on a link that resumes, while `Departed` is resent
   from the outbox, so the edge closes the player's menu, with `container_close`, when it
   handles a `Departed` (a hand-over always closes it). After a `Welcome::Resumed` the
   edge sends each of its players' regions a numbered `PlayerInput::MenuResync`, which the
   region answers with the menu whole or with none; as an input it keeps the region
   deterministic.
8. **The ender chest across a line is any other container** (M4). Section 6 item 12 is
   withdrawn: the block, its lid, its openers and its recheck are its holder's, so it
   opens as item 4 says, by a lend, and its slots are the player's own as decision 5 has
   it.
9. **The format is raised, not nested** (L1). The log's commit kinds nest, each holding
   the fields of the one before, so a kind for stays with an ender chest would hold every
   older kind's too. The format version is raised instead and the place's layout changed
   everywhere, the players' file with it; worlds made before are refused and made again
   (section 6 item 7). Section 6 item 8's "the old kinds read with an empty one" is
   withdrawn.
10. **Leaving with a carried stack drops it**, as the game does for a removed player
    (`dropOrPlaceInInventory`, brief 3.18), at the player's position, before their place
    is written. Section 6 item 2's "put back" is withdrawn for leaving; a hand-over still
    puts it back, as the game's `closeContainer` does for a player who stays.
11. **The explosion's effect** (L3) says whether the owner is a creative player, not only
    a player (vehicles and hanging things read it, brief 2.0 item 7), and its explosion
    id is the exploding entity's key and the tick, unique across regions, so two regions'
    explosions in one tick never merge their drops.
12. **A player across a line on a plate or a tripwire** (L4). Players are not in copies,
    so the holder's recount would undo an "entity came in" effect for one at once. The
    effect carries the player's box, sent by the player's region every tick they touch
    the block, and the holder's recount counts the boxes it was sent for the tick before
    as well as its own entities and its copies'. Named: a tick late.
13. **A player logged in twice can duplicate items through a container** (L5), as through
    a throw already: what a dead stay puts into a chest after the second login read its
    place is not taken from the place the new stay has. Named, under ADR-0020 section 10.
14. **Reach for dispensers, droppers and hoppers** (L6). Each of the new circuit parts
    has its own reach in `lends.rs` (the block in front, the block below it, a double
    chest's other half), not the two blocks every part has by default, so a hopper near a
    line does not ask for lends it does not need. ADR-0027 section 7's "a circuit's part
    changed" is amended: a hopper, dispenser or dropper whose contents change counts.
15. **On closing, `container_close` goes first** (L7), then the `set_player_inventory`
    packets for the slots the carried stack filled, as `closeContainer` sends them.
16. **Who builds what**:
    - N6a also fixes the openers' count field (item 5), the serial in `ContainerClick`
      and `CloseContainer` (item 2), `PlayerInput::MenuResync` (item 7), the use kept
      for a lend (item 4) and the explosion effect's fields (item 11).
    - N6b's hand-over close, its restore and load reports (items 3 and 6) and the edge's
      side (items 1, 2, 7 and 15) are mine; the menus' clicks, the containers, lids,
      sounds, spilling and comparators are delegated.
    - The lending changes (items 4 and 14, section 6 item 4) and the counting of copies'
      entities and players' boxes (section 6 item 6, item 12) go to N6g, mine, beside the
      explosion across a line.
    - The order stays: a, b, c, then d and e in sequence or side by side, f last; c needs
      b's spilling and typed block entities.

**For the owner, from the second review**: a chest, a barrel, an ender chest or a
hopper's container across a pinned line opens by lending its chunk to the user's region
at the moment of use (item 4), which amends the owner's "lending at build time" of
ADR-0027 with a use of a container as a fourth reason to lend. Without it, a container a
few blocks across a pinned line does not open at all, which a player building a storage
room at the line meets at once. Proposed as item 4 has it; the owner can overrule.

## 8. The third review, and what it changed

A third independent review (2026-10-18) read section 7 against sections 2, 3 and 6,
ADR-0027 and the code. It found items 1 to 3 and 5 to 7 sound but for what follows, and
item 4 not ready. Each change below overrides what it amends above it.

1. **A lend for a use is given back** (item 4). The borrower looks at a lent chunk for
   giving back only when a circuit's part near it goes (`part_gone_near`) or at a restore,
   so a chunk lent for a chest would never go back. Now a use's ask is asked once, never
   again every 20 ticks, and is dropped when the kept use ends; the chunk is put in
   `lent_to_check` when a menu on a container in it closes, and when a grant comes with
   no kept use for it; and `give_back_lent` (not `must_keep_for_now`, which is the
   lender's test, as item 4 said wrongly) keeps a chunk while one of the borrower's menus
   shows a container in it. So a lend for a use lasts while the menu is open.
2. **The lender does not lend a container its own player has open** (item 4). The
   lender's `must_keep_for_now` answers "wait" while any of its players' menus shows a
   container with a block in the asked chunk, a double chest's other half among them.
   Two friends at one chest by the line: the second waits for the first to close it.
3. **The kept use** (item 4). A player has at most one. It is dropped by any later
   `UseItemOn`, `Interact`, `OpenVehicleInventory` or `CloseContainer`, by their
   hand-over, and by a merge, a split or a restore of their region; its block sequence is
   acknowledged at once; and it is judged again, in the tick the lent chunks are loaded
   (not at the grant, when they are held and not loaded: ADR-0027's lost lever), as a
   use: still a container, within reach, not blocked. The worker does not hold a use's
   ask in `waits_for_its_chunk`, since the region keeps the use itself, so a second click
   meanwhile stalls nothing behind it on the link.
4. **A use asks only from the region's own pinned land, and for a short while** (item 4).
   A player standing in lent land does not borrow the next chunk (ADR-0027 section 2:
   lends do not creep). A use is not refused by a "not lendable" note of a circuit's
   (`NOT_LENDABLE_FOR`, a minute); a use refused is noted for 40 ticks only.
5. **The hopper at the line is named** (item 4). A hopper of one region that borrowed the
   chunk of the chest in front of it keeps that chunk while the hopper stands, so the
   lender's players cannot open that chest from their side while it does. Named, and in
   what to try with `--pin`.
6. **Clicks are held, not refused, while their container's chunks load** (item 3). After a
   merge, frequent where regions follow players, a refused click would undo the player's
   sorting. The worker's `waits_for_its_chunk` counts a `ContainerClick` and a
   `CloseContainer` as being about the chunks of the open menu's container (the runner
   reads the region's player), and holds them as it holds a use, until they load. The
   refusal with no slots stays only for a click that reaches the region anyway.
7. **Departures are worked out after the drivers, before `keep_events`** (item 6). A
   passenger's position changes in the entity phase, after the block-event phase, so
   departures worked out before it would miss a rider whose vehicle crosses with their
   menu open. The region works them out, and closes their menus, after `tick_drivers` and
   before `keep_events`: every move is done, and the events of `stopOpen` are still kept
   with their chunk.
8. **The edge's copy stays true** (item 1). The region reports, every tick, the open
   menu's slots that changed against what it last reported, as `broadcastChanges` does,
   not only after clicks; and the edge maps a `PlayerEvent::InventorySlot` onto the menu's
   slot for it while a menu is open, in the copy and the comparison. An answer is compared
   on the slots its hashes touched and the slots it changed.
9. **The edge's threshold** (items 2 and 7). The edge keeps the higher of the last
   opening's and the last closing's input numbers; a menu event whose last input applied
   is lower is dropped, so a restore to before a close cannot open the old menu again. At
   a whole report the kept hashes up to its last input applied are dropped and the remote
   slots set to the copy. `container_close` at a `Departed` is sent only while the edge
   believes a menu open (it closes whatever screen the client shows, the player's own
   inventory among them). `MenuResync`'s answer of none carries the inventory whole, as
   a resume may have lost `MenuClosed`'s slots. Menu events are taken only from the
   player's own region (`view.region`).
10. **Plates' boxes are keyed by the effect's due tick** (item 12), not "the tick before",
    so jitter in the coupling does not flicker a plate.
11. **Stale text**: decision 10's and section 4's "a menu across a pinned line does not
    open" are replaced by item 4 with these changes. Section 4 gains: a use across a
    pinned line waits for its lend, up to 40 ticks; a container the lender's player has
    open, or a hopper of the other side has borrowed, does not open from the other side;
    a menu closes at a line; leaving drops the carried stack where the game does too.

## 9. The fourth review, and what it changed

A fourth independent review (2026-10-18) read section 8. It found items 7 and 8 sound,
and three serious defects in the lend at the moment of use, each in ordinary play at a
pinned line: asked only once, a use failed whenever the lender answered "wait" (one of
its players standing anywhere in the chunk); given back at every close, a chunk on its
way back refused a quick second click and made ADR-0027's unmended gap routine, with a
whole lend (a save, an unload, neighbours stopping, every viewer served again) for each
open and close; and a short lend for one chest fixed the way of its stretch, so that the
other side's circuit asked once and was refused for a minute. Its mending would have
added a grace period, a mended gap in the store, a third kind of answer and a list of
asks of their own. Four reviews of that mechanism have each found deeper defects than
the last, so it is withdrawn, and with it the owner keeps ADR-0027 as they chose it.

1. **Containers are lent at build time, as circuits are** (withdrawing section 7 items 4
   and 8 and section 8 items 1 to 5). A chest, a trapped chest, a barrel, a shulker box
   and an ender chest join the circuit's parts of ADR-0027 section 2 beside the
   dispensers, droppers and hoppers of section 6 item 4, with the reach of a container
   (its own block, and a double chest's other half). So a container placed by a player
   near a pinned line asks for the land it reaches into, and lives in the builder's
   region, which runs its menus as anywhere else; it is kept while it stands, as any
   part is. No use asks for a lend, no use is kept, and the worker holds nothing new.
2. **A menu opens only where the player's region holds every chunk of its container**, as
   decision 10 has it. A container across a pinned line that its user's region does not
   hold does not open (named): one the other side built, one placed where the lend was
   refused, one of the lender's that a hopper of the other side borrowed. The ender chest
   is any other container (section 7 item 8 stands).
3. **What stays of sections 7 and 8**: every item but those withdrawn above, with these
   changes from the fourth review:
   - a menu click is held by the worker (section 8 item 6) only while the link waits for
     a subscription of the menu container's chunk, or the chunk is lent and not loaded;
     otherwise it goes through, and a region whose container is not loaded refuses it and
     reports the menu whole when the chunk loads, so a held click never freezes a link
     whose chunk is not coming;
   - the edge takes a menu event from a region that stands for the player's region
     (`living(from) == living(view.region)`), not only from `view.region`, and sends a
     `MenuResync` after it moves a stay (`move_stay`), so the report that heals after a
     merge is never dropped;
   - the lender's wait before lending (ADR-0027 section 3 step 2) also covers a chunk in
     which one of its players' menus shows a container, block or vehicle, so that a lend
     for a circuit never closes a menu open on the lender's side.
4. **Section 4 gains**: a container across a pinned line opens only on the side that
   holds it; a menu closes at a line; leaving drops the carried stack, as the game does.
   And what to try with `--pin 4 --reshape by-hand`: a chest placed at the line opens for
   its builder; a hopper line across the line feeds a chest; a chest the other side
   placed does not open from this side.

**For the owner**, replacing the proposal of section 7: containers follow your "lending
at build time" of ADR-0027 as circuits do. The difference you would meet is that a
container across a pinned line opens only for players on the side that holds it.

## 10. The fifth review, and what it changed

A fifth independent review (2026-10-18) read section 9. Making containers circuit's parts
would trap lends: any part *in* a chunk keeps its lend (`give_back_lent`), so a lever two
blocks from the line, once broken, would leave the other side's storage room lent away
for good. And two chests side by side across a line would make a double chest that
neither side holds whole. The fixes take machinery away.

1. **Containers are not circuit's parts** (withdrawing section 9 item 1). A chest, a
   trapped chest, a barrel, a shulker box and an ender chest ask for no land and keep no
   lend by standing anywhere. Dispensers, droppers and hoppers stay parts (section 6 item
   4), and keep a lend only by their reach leaving their own chunk (the block they face,
   the block below a hopper), never by standing in it; ADR-0027 section 4 is amended so.
2. **Two chests across a line stay single.** A chest does not join a chest in a chunk its
   region does not hold, at its placement or at the shape update that crosses as an
   effect: it is placed single, as a sneaking player places one. So every container is
   whole in one region, and decision 10's "a menu opens only where the player's region
   holds every chunk of its container" never leaves a container that nobody can open.
   Named: "two chests across a pinned line stay single".
3. **A container across a pinned line opens only from its side** (named; for the
   owner). A player whose region does not hold a container's chunk uses it as a remote
   action, which opens nothing (section 6 item 2): a chest one step across the line, even
   one they placed themselves from this side, opens only once they step over. Where
   regions follow players this does not arise (they merge first).
4. **A shulker box facing across a line** reads the copy for room to open and does not
   push the other region's entities (named), as a piston at a line reads one.
5. **Contents and the circuit hooks.** `part_gone_near` and `circuit_changed` fire on a
   block's state; a dispenser, dropper or hopper whose contents change calls
   `circuit_changed` itself (N6a fixes the hook; section 7 item 14). Opening a container
   does not count, except a barrel's `open`, which is a state and counts as any state
   does. Where regions follow players, a busy hopper at a line keeps their split away
   (ADR-0027 section 7), as active redstone does.
6. **The lender's wait** (section 9 item 3, third bullet) covers either half of a double
   chest whose block is in the asked chunk, and a container vehicle whose entity is in it
   at the ask.
7. **Text still in force that this replaces**: section 7 item 16 no longer gives N6a
   "the use kept for a lend" nor N6g "item 4"; section 8 item 11's "replaced by item 4"
   and "waits for its lend, up to 40 ticks" are withdrawn, and section 4 has instead
   items 2 to 4 here. The worker's held click (section 8 item 6, bounded by section 9
   item 3) is the one new hold in `waits_for_its_chunk`. N6e makes dispensers, droppers
   and hoppers circuit's parts with their reach in `lends.rs`, beside their ticking;
   N6g, mine, the amendments of ADR-0027 and the lender's wait. A held click waits, as a
   held use does today, with everything behind it on its link, until the chunk it waits
   for loads (named among the pauses of a merge).
8. **What to try with `--pin 4 --reshape by-hand`** (section 9 item 4 corrected): a chest
   placed while standing on its side opens; one placed a step across the line opens only
   from over there; two chests placed across the line stay single; a hopper line across
   the line feeds a chest.

**For the owner**, replacing the proposals of sections 7 and 9: a container a step across
a pinned line opens only from its own side, as no menu spans two regions. Making it open
from either side needs either a lend at the moment of use, which four reviews found
unsound, or a menu run across two regions, a design of its own. Proposed as named; the
owner can ask for either.
