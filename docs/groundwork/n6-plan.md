# Step N6: TNT, explosions, containers that open, and the redstone that waits for entities, planned

Draft of 2026-10-18, revised the same day after an independent review (section 6, which
overrides what it amends above it); to be reviewed again before it is built. Step N6 of the
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
