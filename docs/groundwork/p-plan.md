# P: the Nether and the End

Draft of 2026-10-29, revised the same day after an independent review and again after
its second reading (`p-review.md`, cited as R: R D5 is its defect 5, R G1 its gap 1, R N3
the second reading's defect 3; section 13 says what changed and where), for the owner's answers before anything is built. The game's rules
are those of `p-brief.md` (cited as G: G 2.3 is its section 2.3, G 0.4 finding 4 of its
section 0), read from the 26.3 jar's classes dumped in `/home/claude/reference/d-javap/`
and the earlier dumps. What Clustine has and where it assumes one dimension is
`p-survey.md` (cited as C: C 1.4 is its section 1.4). Code is cited as it is at
`45350c3`. N10 is done; this is the roadmap's next item ("then the Nether and the End").

Name: the terrain plan's D1 to D4 are taken, so the steps are P0 to P12.

What a world keeps that P changes: a chunk's key, a region's, a player's place and
respawn point name their dimension; the table has a home region per dimension; the
store's format, the worker's `STATE_FORMAT` and the generator's identity rise. **Worlds
made before are refused**, as at every raise before, unless the owner answers otherwise
(question 3). That includes the owner's own play world.

---------------------------------------------------------------------------------------

## 0. What is decided, and why (the lead's proposals; section 12 asks the owner)

1. **A region holds chunks of one dimension, and never of two.** Every chunk-keyed map
   of the simulation stays keyed by `ChunkPos`, since a region's dimension qualifies all
   of them (C 1.2). What a region is (`RegionInfo`, `RegionConfig`, `Restored`) names its
   dimension once. A change of dimension is always a hand-over between two regions.
2. **Each dimension has a home region**, made with the world, never absorbed, holding no
   pinned land. It does for its dimension what the overworld's home does today: it
   places a returning player whose place no region holds (ADR-0020, step 7, case 2) and
   takes land of its dimension that nobody holds (ADR-0028's grant of unheld land, for
   an arrival from another dimension). No region is made on demand: there is no
   store-to-coordinator signal, no wait for a first travel, and no race of a made
   region's id with a split's (R G1, R D1, R D2, R D12). An idle home costs a worker an
   empty region, as the overworld's does.
3. **The store and the edge, which see every dimension, key chunks by dimension and
   position.** A new `LevelChunk { dimension, position }` (in `clustine-world`) keys the
   store's chunk files, grants, owners, copies, lends, arrivals, effects, faces, spawn
   counts and published points of interest (C 1.3), and the edge's replica, players'
   views and shown entities (C 1.4, R D8). Messages on a region's own link keep
   `ChunkPos`, qualified by the region's dimension; messages that can name another
   region's chunk (a claim, a lend, an arrival, `Elsewhere`, a remote action and its
   answer) carry `LevelChunk` (R D9).
4. **`Dimension` is one type** in `clustine-world` with three values, numbered as the
   generated entity keys already number them (overworld 0, the Nether 1, the End 2;
   `generation.rs:317-326`), and the same as `clustine_terrain::biome::Dimension`, which
   becomes it. Its rules are the 26.3 dimension type and its environment attributes
   (G 0.1, G 1.1), emitted whole by datagen.
5. **One world clock**, as the game's game time is one; the Nether has fixed time and no
   clock, and the End has its own clock (G 1.1). Weather is the overworld's alone
   (`canHaveWeather`, G 1.2).
6. **A traveller is committed when the portal fires.** In the game, an entity whose
   portal fires travels whatever it does next (G 2.1, step 3), and a creative player's
   portal fires on their first tick inside it. So the region lets the traveller go in the
   tick it fires, durably, towards the destination dimension (section 5). Only removal
   before that commit cancels it. This is not a request and an answer that the traveller
   could walk out of (R D5, R D6). **A player goes by the edge, as every hand-over of a
   player does** (ADR-0020): a `Departed` to the destination dimension's home, whose
   transfer carries the travel. **Any other entity is filed** as a `Traveller` for the
   target chunk through the arrivals machinery (ADR-0028). One path each, never both
   (R N1). A player waits, with their moves held back, until the destination places them
   (4.2).
7. **The destination places the traveller, finding or making the portal there.** It takes
   the traveller (a player's arrival, or a filed `Traveller`) once, in the same commit as
   the blocks of a portal it makes and the traveller's entity. Restored after a crash, it
   makes no second portal and places nobody twice (R D6). What it waits for before that
   commit (5.2) is not durable: a take-over starts the wait again.
8. **The portal search reads an index kept from every commit**, not from saved chunks.
   The store keeps, per dimension, the positions of `nether_portal` blocks from the block
   changes of every commit it takes. The index is as durable as the log and as current as
   the tick (R D7). The destination adds its own live points of interest to what the
   store says, so two travellers placed one after the other find each other's portal.
9. **The flat world gets the Nether and the End of seed 0**, as the game's flat world has
   them (question 4). **The dragon's first fight is in the phase**, since the dragon is
   what a player sees on arriving in the End (question 5).

## 1. What a player sees when P is done

They light an obsidian frame with flint and steel or a fire charge, step in, and after
the game's wait (none in creative, 80 ticks in survival; G 2.1) the screen turns to the
Nether. It is the Nether of their seed (seed 0 in the flat world), with its fungi,
deltas, basalt and blackstone, glowstone, fortresses, bastions and fossils. A portal is
made for them where the game would make it, or they come out of the one already there.

In the Nether:
- water evaporates when poured, lava runs fast, beds explode;
- a respawn anchor charges and sets their spawn;
- there are zombified piglins, ghasts, magma cubes, blazes and wither skeletons, striders,
  piglins that barter, and hoglins.

They go back through the portal and come out where the game puts them.

Eyes of ender lead to a stronghold, and a filled frame opens the End. They arrive on the
obsidian platform before the dragon, with its crystals on the spikes. They can fight it,
see it die, take the egg, and go home through the exit portal and the credits. Gateways
lead to the outer islands, with their End cities and shulkers.

Mobs, items, boats and mounts go through portals too. All of this works across regions
and workers; a dimension's regions follow their players as the overworld's do.

### 1.1 What they do not see, named

- **The arrival is a few ticks late.** It comes a few ticks after the portal fires, not
  in the same tick (0.6): longer where the target chunk is not the home's and the player
  is passed on to its holder (R N2), and longer still where the portal's chunks must be
  generated first. Whether the client shows its loading screen in that gap or an empty
  level is to be seen (4.2, R N3).
- **Some of the fight comes last.** The dragon's respawn by four crystals, and the rarer
  cases of a fight across restarts (G 8, D6b), come last in the phase and may slip to a
  later one (question 5).
- **No wither in P** (question 5).
- **The End's void** (question 7). Clustine has no death (ADR-0020, section 4), and the
  game's `fell_out_of_world` damage reaches creative players. Without it, a player knocked
  off the island falls for ever.

## 2. What G changes in C, in one place

- The dimension type's old fields are environment attributes in 26.3 (G 0.1); C 4's table
  maps them.
- Portal travel in the game reaches 128 blocks into another level in one tick (G 0.3);
  decisions 6 to 8 are Clustine's answer.
- The client is not waited for after a portal (G 0.5): there is no `PlayerLoaded` wait.
  Movement is ignored until the teleport is accepted, which the edge already does
  (`fanout.rs:76-82`).
- The End has a sky; the worldgen scheduler's `has_sky` says it has not (C 3,
  `pool.rs:463-467`). This is a defect, mended in P1.

## 3. Levels in one world (P1 to P4; the contracts are the lead's)

### 3.1 The data (P1)

- Datagen emits the whole dimension type with its attributes (G 1.1's two tables) into
  `clustine-data`, and a test compares the generated table with the JSON.
- `Dimension` (0.4) gets `rules()`, and the scheduler's `has_sky` reads it.
- The classes G 9 says to dump before building are dumped by the lead, and the brief is
  completed from them before the steps that read them: `EndPlatformFeature`,
  `EndPodiumFeature`, `LavaFluid`, `BlockUtil.getLargestRectangleAround`,
  `ServerLevel.findFreePosition`, `CommonPlayerSpawnInfo`, and the `Respawn` packet's
  wire form, which is inferred today. Most are dumped already, in
  `/home/claude/reference/d-javap/`.

### 3.2 The contracts (P2; the lead's, not delegated)

Everything a later step needs, so that P2 is not reopened (R G2). Each later step that
still changes a shape says which version it raises.

- **Regions.**
  - `RegionInfo`, `RegionList`, `Restored`, `RegionRoute` and `RegionConfig` name the
    region's dimension.
  - `RegionList` has a home per dimension.
  - `RegionConfig.spawn` and `Restored.spawn` are optional, and the overworld's alone; the
    store's test for the chunk players enter by compares a `LevelChunk` (R D4).
  - `RegionConfig` has the dimension's light (`sky`) and floor.
  - The dimension is mixed into the random ticks' seed (R G4).
- **Players.**
  - `Place`, `Respawn` and `LoggedPlace` name a dimension. A stored place below its
    dimension's floor is "no place".
  - `PlayerTransfer` gains `dimension`, and an optional `Travel` (what the `Traveller`
    below carries for an entity); its riding's vehicle chunk is a `LevelChunk`.
  - A player's stay keeps a pending `Travel` until a region places them, so that one
    who leaves before then is placed by the search when they come back, not at the
    target point inside the rock (R N4).
  - `PlayerEvent::Placed { pose, flying, … }`, from the region that places a travelling
    player, after its commit: the word that starts the second half of the edge's change
    (4.2, R N3).
  - The credits: a player's `in_credits` flag in the End region's state, and
    `seen_credits` with the player (5.3, R N8).
  - `WorldPlayer` names a dimension.
- **Travel.**
  - `Traveller`, a filed record of an entity other than a player, with its riders that
    are not players.
  - It carries the destination dimension and the target point, plus the source portal's
    axis and the traveller's place relative to it (G 2.2, item 6).
  - It carries what kind of travel it is (a Nether portal, the End's portal in or out,
    a gateway, or a respawn after the credits).
  - It is filed for the target `LevelChunk` through the arrivals machinery (ADR-0028,
    section 5).
- **The portal clock** in an entity's state: `PortalProcessor` and the cooldown.
- **The portal index** in the store's log (records and the query).
- **The edge's view of entities.**
  - The dimension is in `Shown`, `HandedOn`, `EntityGone`, `RemoteAction` and
    `RemoteDone`.
  - `EntityGone` gains "travelled", with the entity's hops (R D8, R D9).
- **The generator.** `TerrainGenerator::new(seed, dimension)` and the three-dimension
  identity are fixed here, since P3 and P5 both need them (R section 3).
- **The homes** in the table and in the coordinator's configuration, one per dimension.
- **The dragon fight's world record**, and a run of consecutive wire ids for an entity of
  several parts (the dragon's eight, G 6.4), in the edge's `WireIds`.
- **Versions.** `STATE_FORMAT` goes from 24 to 25 and `FORMAT_VERSION` from 18 to 19; the
  byte-shape test and the format round trips say what changed.

P2 is pushed before any subagent starts (CLAUDE.md: worktrees start from `origin/main`).

### 3.3 The store (P3; the homes and the portal index are the lead's)

- Chunks are kept under `chunks/<dimension>/`; the format already has the room (C 1.3), so
  only the constant goes.
- Every chunk-keyed map is keyed by `LevelChunk` (C 1.3's list).
- **Pins are the overworld's.** `holder` and `pinned_holder` look at pinned areas only for
  overworld chunks (`table.rs:363-369`, R D3).
- **Generators.** There is one generator per dimension, built from the world's seed. The
  generator's identity names all three, so every world's number changes (question 3).
- **The three homes are made with the world**, in the table's first record. Each home
  takes unheld land of its own dimension, as the overworld's does. An arrival from
  another dimension into land no region holds goes to that dimension's home, not to the
  sender (R D12).
- **The portal index (0.8)** is kept from every commit's block changes, per dimension, and
  queried by square. It is part of what a checkpoint keeps, so the log's compaction does
  not lose it (R second reading, D7).
- **The table made anew** when a world's division changes (`Table::made_from`) makes all
  three homes; the load-time check of the home chunk is the overworld's (`table.rs:282`;
  R N9).
- Mob caps, the world-player list and faces are kept per dimension.
- Every one of these changes has kill tests at every step, as before.

### 3.4 The coordinator (P4)

- **No merge across two dimensions**, by either path:
  - the distance between two dimensions is infinite (`policy.rs:50`, `:161`);
  - absorbing an empty region chooses a survivor of the same dimension only
    (`follow.rs:1060-1072`).
- **A home is never absorbed.** Each dimension's home is the survivor of first choice in
  its own dimension, as the overworld's is now.
- **The store refuses** an `AbsorbCommit` across two dimensions, as a second line of
  defence (R D1).
- The places that `policy::places` adds for a home are in that home's dimension.
- When a Nether or End home splits, the group holding the dimension's chunk (0, 0) stays
  in the home, as the group holding the entry chunk does in the overworld
  (`policy.rs:306-311`; R N9).
- Pins are the overworld's.
- The absorption change is ordering-critical, and is briefed precisely or done by the
  lead (R section 3).

### 3.5 Serving the land (P5, P6; delegable, one subagent per batch)

- **Per dimension:** `TerrainGenerator::new(seed, dimension)` (fixed in P2), the features'
  stages and `makes_mobs`; sea level comes from the noise settings (C 2).
- **F7's twelve feature kinds** (C 2): the Nether's first (deltas, blobs, fungi,
  spreading), then the End's (spikes, platform, podium, islands, chorus, gateways).
  - A spike's End crystal and the End cities' shulkers are entities of P12.
  - The blocks come in P5 and P6; the entities follow with P12's families.
  - `ENTITIES_GENERATION` rises after those families (R section 3).
- **The structures' placers:** the fortress, the Nether fossil and the End city (S3's
  rest; C 2), and the bastion checked block for block.
- **Judging.** They are judged by the game's own classes, as the overworld's are, with
  `game_features.rs` extended to the Nether and the End.
- **The fallbacks at −64 and 319** (`falling.rs:605`, `items.rs:152`, `boats.rs:646`)
  take the dimension's floor and ceiling (R G10).

## 4. The edge (P7; the lead's, not delegated)

### 4.1 Keyed by dimension

- The replica is keyed by `LevelChunk`. Every player's view, and every shown entity
  (`Shown`, `fanout.rs:981-993`), names its dimension (C 1.4, R D8). A region's link
  reports chunks and entities of the region's dimension.
- **Positional content of another dimension is dropped.** Content whose dimension is not
  the view's is not shown: a remote action's shown blocks, a late delta, a snapshot
  (R D9). A remote action's answer itself is still taken, so that the client's later
  acknowledgements are not held back behind it (`fanout.rs:3185-3209`; R N6).
- **A travelling player is removed from other screens.** Players are shown by the player
  rule and not by `EntityGone` (ADR-0028, section 6), so the edge that sees a player's
  change of dimension tells its other players of the old dimension that the player is
  gone, and every other edge hears it as it hears a player who left (R second reading,
  D8).
- **The login and the protocol.**
  - The login names the stay's dimension, and all three in `dimension_names`.
  - `LoginWorld` is per dimension: sea level, and `is_flat` for the flat overworld only.
  - `Respawn` goes into `clustine-protocol`, with `CommonPlayerSpawnInfo` (G 3.1, item 2).
- **Light.**
  - Light jobs take the dimension's `sky` (`fanout.rs:5476`).
  - Where there is no sky, the fallback light over heights has none either (C 3).
- **The bots** learn `Respawn` and follow a change of dimension (`bot.rs:141-150`).

### 4.2 The change of dimension

**What triggers it.** A departure whose transfer's dimension differs from the view's.
That holds for a `Departed` turned back from a `NotMine` too, which so does not send a
second `Respawn` (R D10, `fanout.rs:11437`).

**The order.** The edge does its own notes first and tells the new region before the
client hears anything, for the reason `enter_by_arrival` records (`fanout.rs:4698-4704`):
1. Unwant the old view, by `LevelChunk`.
2. Switch the view's dimension.
3. Clear `sent`, `pending`, `replace`, `visible` and the riding state. The client drops
   its level and its entities with the `Respawn`; nothing is unloaded or removed one by
   one.
4. Reset `told_clocks`.
5. Want the new view of the destination's region.
6. Send `PlayerArrive` with the transfer, which carries the `Traveller`.
7. Send the client `Respawn(keep 3)`, then the difficulty and the operator level
   (G 3.1, items 2 to 4), and hold the client's moves back from here, as while a
   teleport is awaited (`awaiting_teleport` set to a value no teleport has; R N3).

**Between the halves.** The client may show its loading screen or an empty level: the
brief says `LEVEL_CHUNKS_LOAD_START` (G 3.1, item 13) opens the screen, and the client's
handling of `Respawn` is not dumped. P7 begins by finding out, with the official server
and a real client (the owner's), and sends that event with the first half if the gap
would otherwise show an empty level. A travelling view has a patience of its own, as a
join has (20 s): a player not placed in it is put out with the reason, and their stay,
durable in the destination dimension with its pending travel, places them when they come
back (R N3).

**When the destination places the player** (section 5.2) and says so with `Placed`, the
edge sends, in G 3.1's order:
1. The position, with the game's relative flags (`DELTA ∪ ROTATION`, a yaw of 0 or 90)
   and a new teleport id that the client must accept.
2. The abilities.
3. The level's information, ending with `LEVEL_CHUNKS_LOAD_START`.
4. The inventory and the held slot.
5. The effects and the post effects.
6. The portal sound.

Then the chunks follow.

**Inputs.**
- Kept inputs from before the change are replayed only where they have nothing to do with
  position: a held slot, a creative slot, a swap of hands, flying (R D11).
- Moves, and actions aimed at blocks or entities, are dropped.
- The inventory sent comes from the destination's entity, which has had the replayed
  inputs.
- Actions aimed at blocks that come after the `Respawn`, and before the new teleport is
  accepted, are dropped as moves are.

**A player who leaves**, or whose edge restarts, during the change. Their stay is in the
destination dimension from the source's commit, with its pending travel (3.2). A new
login finds them through that dimension's home: ADR-0020's step 7, case 3, with the home
of the place's dimension as the region to ask (case 2 would put them in the overworld's
home; R second reading, G1). The home runs the search again and places them (R N4).

## 5. Travel (P8, P9; the travel is the lead's)

### 5.1 The portal clock (the region's; delegable once P2 has fixed what firing does)

`PortalProcessor`, the cooldowns, and who may travel, as G 2.1 and G 2.7 say, all in
the region the entity is in.

When the clock fires, the region does what 0.6 says, in that tick and in one commit.
**That tick is the lead's** (P9); P8 builds the clock and the blocks (R second reading,
section 3).
1. It removes the traveller.
2. For a player, it puts a `Departed` in the outbox, to the destination dimension's home,
   with the travel in the transfer. The source cannot name another region of the
   destination: it learns nothing of another dimension's holders within a tick. Where
   another region holds the target chunk, the home passes the player on with `NotMine`,
   one hand-over more, inside the wait (R N2).
3. For any other entity, it files the `Traveller` for the target `LevelChunk`, and reports
   to the edges a removal for travel, with the entity's hops, so that no other client
   keeps a still copy at the portal (R D8). The game sends the old level's watchers a
   removal too (G 2.7, G 3.1 item 5).

**Riders.** One root travels with its unit: the vehicle carries the player (G 2.7). A
vehicle with a player in it goes with the player, in the player's transfer (as riding
crosses a line today, ADR-0028), not as a filed `Traveller`; so both arrive at one region
by one path. The vehicle's placing decides the player's pose (the seat's offset), and
`Placed` comes once both are placed. The client, which dropped the vehicle with the
`Respawn`, is told `SetPassengers` once the vehicle is introduced (R G7, R N7). A vehicle
with no player in it is filed with its riders.

### 5.2 Placing a traveller (the lead's)

1. **Taking the traveller.** The region that holds the target chunk takes it once: a
   player's arrival as any arrival is taken, a filed `Traveller` through the arrivals
   machinery (in its commit, at its hello after a take-over, and carried with the land
   through a merge or a split).
   **What it waits for first, not durably** (R N5): the store's answer from the index;
   the spiral's chunks loaded (generated if they must be); border copies; and grants for
   unheld chunks it claims. A take-over during the wait starts it again at the hello.
   Two travellers ready in one tick are placed in order, the second seeing the blocks
   and points of interest the first set.
2. **Finding a portal.** It asks the store's index for the dimension's portal blocks
   within the radius (16 or 128; G 2.2), and adds its own live points of interest. It
   keeps those whose block is still a portal, and chooses as G 2.2 does: nearest by
   `distSqr`, then the lowest. Whether the block is still a portal can be checked only in
   its own chunks and copies; farther candidates are taken on the index's word, which is
   current to the last commit (R N5).
3. **Reading the rectangle.** It reads the chosen portal's rectangle (G 2.2, item 4).
4. **Making a portal** if none is found (G 2.3). The search's blocks and heightmaps come
   from its own chunks and from border copies of the others (ADR-0025, section 3). The
   portal's blocks are written in its own chunks. Blocks in chunks another region holds
   are filed as effects (ADR-0025), and the region claims chunks that no region holds
   (R G6). The End's platform at (100, 49, 0) is made the same way (G 2.4). Named, both
   rare: a block in another region's chunk that changed meanwhile is skipped by its
   effect (`SetIf`), which can leave a frame open and its portal blocks to break; and two
   travellers placed by two regions within 16 blocks of each other, both before either's
   commit, make two portals (R N5).
5. **Placing the traveller** at G 2.2, item 6's pose, with its velocity and rotation, all
   in the same commit as the portal's blocks.
   - If the arrival chunk is another region's, the traveller is handed over to it as an
     ordinary walk is.
   - For a player, the region's `Placed` gives the edge the pose to send (4.2).
6. **Keeping chunks loaded.** The game's portal ticket (radius 3, G 2.2) keeps the
   arrival's chunks loaded. In Clustine the arrival region holds them, as for ADR-0028's
   arrival ticket (R G5).

Every step is in the log of the region or of the store, so each survives a take-over of
either region and a restart of the store. The tests kill every process at every step, as
N3's did.

### 5.3 The End's portal, the credits and gateways (P10)

As G 2.4 to G 2.6.

**Leaving the End.** Leaving by the portal, or after the credits, uses the player's bed
or anchor without spending a charge (G 2.4, G 2.5). That is a read of a block in another
region, perhaps in another dimension. The `Traveller` goes to the region that holds the
respawn block. That region checks the block and places the player there, or, if the block
is gone, at the world spawn with `NO_RESPAWN_BLOCK_AVAILABLE` (R G8).

**The credits.** The player stays in the End region, hidden and still, with an
`in_credits` flag in the region's state, so that their stay keeps a region that acts for
it and answers for it at a hello (R N8):
1. The region sends `GameEvent(WIN_GAME)` (G 2.5), shows the player to nobody, and
   ignores their inputs but `PERFORM_RESPAWN`.
2. On `PERFORM_RESPAWN` the End region departs them, as at a portal, with the respawn's
   travel (keep 1, G 3.2), to the region that holds the respawn block or to the
   overworld's home.
3. A player who leaves during the credits enters next time at their respawn, as the game
   would after the credits.
4. `seen_credits` is kept with the player.

**Gateways.** Their exits are searched in the End's chunks 1024 blocks out (G 2.6),
through the store's test for an empty chunk. The island is made by the region that holds
the exit.

## 6. Rules that differ by dimension (P11; delegable by family after P2)

From G 5 and C 4's table:
- **Water and lava:**
  - water evaporates (bucket, dispenser, ice, sponge, dripstone);
  - lava runs fast.
- **Beds and anchors:**
  - beds' rules by dimension, and a bed's explosion;
  - respawn anchors: charging, setting the spawn, exploding outside the Nether.
- **Fire:** infiniburn by dimension, and soul fire.
- **What is off outside the overworld:**
  - raids are not started in the Nether;
  - the custom spawners run in the overworld only.
- **Light and spawning:**
  - the monster spawn light by dimension;
  - the region's light without a sky (`region/light.rs:100`, R G3);
  - darkening fixed in the Nether and the End.
- **Heights:** navigation heights and the sea level by dimension.
- **Mobs:**
  - endermen never flee daylight in the Nether and the End;
  - snow golems melt;
  - piglins and hoglins zombify in the overworld;
  - zombified piglins come from portal blocks in the overworld.
- **Pearls:** a pearl that carries its owner into another dimension is a change of
  dimension that no portal made (R G10).

## 7. Mobs (P12; delegable by family, as N7 and N9)

In G 8's order:
1. Zombified piglin.
2. Magma cube.
3. Ghast and its fireball.
4. Wither skeleton.
5. Blaze and small fireballs, with the fortress's spawn list.
6. Strider and its riding.
7. Piglin: its brain, bartering, zombification.
8. Hoglin and zoglin.
9. Piglin brute.
10. Endermite.
11. Shulker and its bullet.
12. End crystals.
13. **The dragon's first fight** (G 6):
    - the fight's saved data, as a world-level record published like N10's raids, ticked
      by the region that holds the End's chunk (0, 0);
    - the boss bar, the phases, the crystals' healing;
    - death, the egg, the exit portal and the first gateway.

    The game keeps the arena's chunks loaded with the fight's ticket (radius 9;
    `isArenaLoaded` needs x −8 to 8 at z 8). In Clustine, the region that ticks the fight
    claims and holds those chunks from when the boss bar first has a player (G 6.1,
    step 3), since the fight cannot begin before they are loaded (R G5, second reading).
14. The respawn by crystals, last.

## 8. What clients are sent

Everything above is sent with the game's packets; nothing is new in kind but `Respawn`.
There is a comparison for each of these:
- the change of dimension (its packets and their order);
- the portal's place on both sides;
- the End's platform;
- water poured in the Nether;
- a Nether chunk's light;
- the dragon's first moves.

## 9. The division of work

| Step | What | Who |
|---|---|---|
| P0 | This plan, the brief, the review, the owner's answers | lead |
| P1 | Dimension data and type; `has_sky`; the dumps G 9 names | subagent once the type is fixed; dumps the lead's |
| P2 | Contracts (3.2), including what firing a portal does (0.6) | **lead** |
| P3 | The store (3.3): layout, keys and generators by a subagent; the homes, the portal index and their kill tests by the **lead** | subagent and lead, in separate files |
| P4 | Coordinator (3.4) | **lead** (absorption is ordering-critical) |
| P5 | Terrain served per dimension; F7 | subagents, one per batch |
| P6 | Fortress, fossil and End city placers | subagent |
| P7 | The edge (section 4) | **lead** |
| P8 | Portal blocks: frames, lighting, breaking, the portal clock | subagent |
| P9 | The tick a portal fires in (5.1), placing travellers (5.2), entities and riders through portals | **lead** |
| P10 | The End's portal, the platform, the credits, gateways | lead for the change; subagent for blocks |
| P11 | Rules by dimension (section 6) | subagents by family |
| P12 | Mobs and the dragon (section 7); the structures' entities | subagents by family; the fight's record the lead's |
| X | Tests from the specification of every step, by someone who did not write it; the comparisons (written by a subagent, run by the lead with `tools/oracles.sh`) | subagents |

**Order.**
1. P1 and P2 come first, and are pushed.
2. Then P3, P5 and P6 side by side: the store and `services/worldgen` are distinct
   crates, and the generator's constructor is fixed in P2.
3. P4 and P7 follow P3.
4. P8 and P11 both touch `clustine-sim`, and share `region/random_ticks/fire.rs`. They are
   given files of their own: P8 the portal block and the clock, P11 everything else, with
   fire's two lines for portals left to P8.
5. P12's families each add a file, and the lead adds the dispatch in `mobs/types.rs`.
6. P9 follows P7 and P8, and P10 follows P9.

Worktrees are bounded by file, not by crate.

## 10. Size

G 8 estimates about 62,000 to 90,000 lines with tests, two to three times N10. If the
owner prefers, P splits into two milestones: the Nether (P0 to P9, P11 and the Nether's
mobs), then the End (P10, the End's mobs and the dragon).

## 11. Cost and measurement

- The arrival's delay after the portal fires: on one machine and across workers, in
  memory and on the real disk.
- A Nether crowded with ghasts and piglins, against the overworld's crowd.
- An idle home's cost on its worker.

## 12. Questions for the owner

1. **One dimension per region, and a home region for each dimension** (0.1, 0.2), with
   travel always a hand-over: agreed?
2. **Arrival after the portal fires.** The traveller is committed in the tick the portal
   fires and arrives a few ticks later, longer where the player is passed on from the
   dimension's home to the region that holds the target, or the portal's chunks must be
   generated. The player waits with their moves held back, on the loading screen or in
   an empty level (to be found out, 4.2). A crash loses nothing (0.6). Is that
   acceptable, or must the arrival be in the same tick? That would need the destination's
   chunks held by the source region in advance.
3. **Worlds made before P.** They are refused and made again, as at every change of
   format, which means **your own play world is lost again**. Or should their overworld
   be kept? Its chunks are already under `chunks/overworld/`; the table, the players'
   file, the state and the world number change.
4. **The flat world** gets the Nether and the End of seed 0, as the game's flat world
   has. Agreed, or no other dimension in the flat world?
5. **The dragon.** Its first fight is proposed for P. Should its respawn by crystals come
   last in P, or later? The wither is proposed for later. Agreed?
6. **One phase or two milestones** (section 10)?
7. **Survival-only parts.** These include a bed's explosion that hurts, an anchor that
   sets a respawn point used at death, and the respawn screen. **Should the End's void
   kill?** The game's void reaches creative players; Clustine has no death yet (1.1).
   Build these now as the game has them, or with survival mode?
8. **Workers.** Do the Nether's and the End's regions run on the same workers as the
   overworld's, placed by load like any region (proposed)? Or on workers of their own?
9. **The terrain plan's D3**, a world of a single dimension chosen when it is made
   (`terrain-plan.md:554`). Drop it, now that the three dimensions come together?
   Proposed: dropped.

## 13. The review (2026-10-29), and what the plan is now

The review (`p-review.md`) found twelve defects and ten gaps. The lead checked the two it
rests most on in the code: absorption's survivor rule (`follow.rs:1060-1072`) and the
pins' fallback in `Table::holder` (`table.rs:363-369`). Both are as it says. What
changed:

- **A home region per dimension** replaces regions made on demand (0.2, 3.3, 3.4). This
  answers:
  - R D1: an empty Nether region absorbed into the overworld's home;
  - R D2: a made region's channel, its wait and its id racing a split;
  - R D12: unheld land granted to a sender of another dimension;
  - R G1: a returning player with a Nether place.
- **Pins are the overworld's** in the store too (R D3). **The spawn is optional, and the
  overworld's** (R D4).
- **A traveller is committed when the portal fires** (0.6, 5.1) and is filed as a
  `Traveller` through the arrivals machinery, taken once, in one commit with the portal it
  makes (0.7, 5.2). This replaces a request and an answer that a creative player walked
  out of (R D5) and that had no channel for mobs and no address for the answer (R D6).
- **The portal index is kept from every commit** and joined with the destination's live
  points of interest (0.8; R D7).
- **No ghost at the portal.** A crossing is reported to the old dimension as a removal
  with hops, and shown entities carry their dimension (5.1, 4.1; R D8).
- **The edge drops another dimension's late content** (4.1; R D9). The change keeps the
  hand-over's order, is triggered by a difference of dimension (4.2; R D10), and replays
  the inputs that have nothing to do with position (R D11).
- **The gaps:**
  - the contracts are completed (3.2; R G2);
  - the region's light without a sky (6; R G3);
  - the random ticks' seed (3.2; R G4);
  - the fight's arena and the portal's ticket (5.2, 7; R G5);
  - a portal made across regions (5.2; R G6);
  - riders (5.1; R G7);
  - the End's exits and the credits (5.3; R G8);
  - the void named and asked (1.1, question 7; R G9);
  - R G10's smaller items, in 3.1, 3.5, 4.2 and 6.
- **The division** is by file, with absorption and the dispatch the lead's (9; R
  section 3).
- **The questions:** question 2 is reframed, question 9 is added, and question 3 says
  plainly that the owner's world is lost.

### 13.1 The second reading

The reviewer read the revision and found it answered most of the first review, and nine
new defects, chiefly in how a player travels. What changed:

- **A player goes one way only**, by the edge, as every hand-over of a player does; only
  other entities are filed as `Traveller`s (0.6, 5.1; R N1). The `Departed` goes to the
  destination's home, which passes the player on when another region holds the target
  (R N2).
- **The edge's change** has its starting word (`Placed`), holds the client's moves back
  between its halves, finds out what the client shows in the gap before it is built, and
  has a patience (4.2, 3.2; R N3). A player who leaves in the gap is placed by the search
  when they come back (R N4).
- **The destination's wait** before it takes a traveller is named, with what it can and
  cannot check and two rare outcomes named (5.2; R N5).
- **A remote action's answer of another dimension** is still taken, only its blocks
  dropped (4.1; R N6).
- **A vehicle with a player** goes with the player, and its placing decides the player's
  pose (5.1; R N7).
- **The credits** keep the player in the End region, hidden and still (5.3; R N8).
- **The homes** split and are made anew as said in 3.3 and 3.4 (R N9).
- **The firing tick is the lead's** (9). A travelling player is removed from other
  screens; the portal index survives the log's compaction; the arena is held from the
  boss bar's first player (4.1, 3.3, 7).

## 14. The owner's answers (2026-10-29)

- **Old worlds** (question 3): refused and made again, as at every change of format.
- **Two milestones** (question 6): **the Nether first** (P0 to P9, P11 and the Nether's
  mobs of P12, with the Nether's land of P5 and P6), **then the End** (P10, the End's land,
  its mobs and the dragon). The contracts of P2 are made for both at once, so that the
  End does not reopen them; the End's home is made with the world from the first
  milestone on, and serves an empty End until the second.
- **The arrival a few ticks after the portal fires** (question 2): accepted, measured and
  written down.
- **Survival-only parts** (question 7): **the End's void kills now**, as in the game,
  with the respawn it needs (death in the void, the respawn screen, the respawn at the
  world spawn or the bed or anchor). It comes with the End's milestone, since only there
  is a void a player meets. A bed's explosion, an anchor's explosion and damage from them
  come with survival mode; until then a bed used in the Nether or the End is removed and
  explodes without hurting anyone, and an anchor used outside the Nether likewise.
- Questions 1, 4, 5, 8 and 9 were put to the owner with the plan's proposals, to be
  followed unless they say otherwise: one dimension per region with a home per
  dimension; the flat world with the Nether and the End of seed 0; the dragon's first
  fight in the End's milestone with its respawn by crystals last, the wither later; the
  dimensions' regions on the same workers, placed by load; the terrain plan's D3
  dropped.

## 15. The contracts as P2 builds them (2026-10-30)

Section 3.2 says what P2 fixes; this is how, read against the code at `7bef868`. One
change from 3.2 is the lead's, and is argued here before anything is built.

### 15.1 What a link's dimension already says

A region holds chunks of one dimension, from its making to its end, and never absorbs a
region of another (decision 1, 3.4). So every `ChunkPos` a region sends or is sent on
its own link is of the region's dimension, and so is every `ChunkPos` of the store's
records of that region. P2 gives the region's dimension once, where a region is named:
`RegionInfo`, `Restored`, `RegionConfig`, `RegionRoute`. Whoever reads a region's
message qualifies its chunks by that.

- **The edge** knows every link's region from its route, and so its dimension. What 3.2
  put in `Shown`, `HandedOn`, `EntityGone`, `RemoteAction` and `RemoteDone` is that
  dimension, which the edge has from the link the event came on (R D8, R D9 and R N6
  are answered by the edge's keys of P7, not by the events). P2 does not add it to them.
- **The store** knows every region's dimension from its table. Claims, lends, arrivals
  within a dimension, `Elsewhere`, effects, faces and counts stay `ChunkPos` on the
  wire, and become `LevelChunk` in the store's maps (P3).
- **What does cross dimensions** names it: a `PlayerTransfer` (its `dimension`
  qualifies every chunk in it, its riding's vehicle chunk among them), a `Traveller`
  (its target is a `LevelChunk`), a `Place` and a `Respawn` (a player's, kept by the
  store across stays), and a `WorldPlayer` (the store's list of every player).

This keeps the change to the wire where a dimension is crossed, and removes a field that
could disagree with the link's region.

### 15.2 The types

**`clustine-world`:**

- `Dimension` (P1).
- `LevelChunk { dimension, position: ChunkPos }`, ordered by dimension and then position:
  the key of the store's and the edge's chunk maps.
- `travel`: what crosses with a traveller.
  - `Travel { to: Dimension, target: Vec3, way: Way }`. `target` is where the game aims
    before it searches: the scaled and clamped point of a Nether portal (G 2.2, 2), the
    End's spawn point, a gateway's exit.
  - `Way`:
    - `NetherPortal { axis: Option<PortalAxis>, relative: [f64; 3] }`: the source portal's
      axis and the traveller's place in its rectangle (G 2.2, 6), which the destination
      places the traveller by;
    - `EndPortal`, into the End or out of it to the respawn;
    - `Gateway`, within the End;
    - `Credits`, the respawn after the credits (5.3).
  - `PortalAxis { X, Z }`.
  - `Traveller { travel: Travel, chunk: LevelChunk, entities: Vec<Entity> }`: an entity
    other than a player, root first with its riders that are not players, filed for the
    target chunk (5.1, 5.2).
- `Entity.portal: PortalState { clock: Option<PortalClock>, cooldown: u32 }`, with
  `PortalClock { kind: PortalKind, entry: BlockPos, time: u32, inside: bool }` and
  `PortalKind { Nether, End, Gateway }`: the game's `PortalProcessor` and
  `portalCooldown` (G 2.1).
- `dragon::DragonFight`: G 6.1's saved fields (`needs_state_scanning`, `dragon_killed`,
  `previously_killed`, the respawn's stage and time, the dragon's key, the exit
  portal's place, the gateways, the respawn crystals' keys). It is kept in the End's home
  region's state (P2 makes the field, P12 fills it).

**`clustine-sim`:**

- `RegionConfig` gains `dimension`; `spawn` becomes `Option<Vec3>`, the overworld's
  regions' alone. `lowest_y` is the floor. The light needs no field: it is
  `dimension.rules().has_skylight`.
- The random ticks' seed mixes in the dimension's number, which is 0 for the overworld,
  so that the overworld's draws stay as they are (R G4).
- `Place` gains `dimension`, `travel: Option<Travel>` and `seen_credits`. A stay let go
  through a portal keeps its travel in its place until a region places it (R N4): the
  store keeps the place as the `Has` says, and the destination clears the travel.
- `Respawn` gains `dimension`.
- `PlayerTransfer` gains `dimension`, `travel: Option<Travel>` and `seen_credits`.
- `PlayerState` gains `portal: PortalState` and `in_credits`.
- `PlayerEvent::Placed { dimension, pose, flying }`, from the region that placed a
  travelling player, after its commit (4.2).
- `WorldPlayer` gains `dimension`.
- `PlayerEvent::EntityGone` gains `travelled: bool`: gone through a portal, which
  clients are shown as a removal, not a death.
- `TickInputs` gains the filed travellers, `(EffectId, Traveller)`, and `TickOutput`
  the travellers a tick sends, `Vec<Traveller>`. A traveller taken is named among the
  arrivals taken: they share the arrivals' numbers.
- `RegionState` gains `dragon_fight: Option<DragonFight>`.

**`clustine-rpc`:** `RegionInfo.dimension`; `RegionList.homes: Vec<(Dimension,
RegionId)>` beside `home`, which stays the overworld's; `Restored.dimension` and
`Restored.spawn: Option<BlockPos>`.

**`clustine-region`:** `RegionRoute.dimension`; `RoutingTable.homes`.

**`clustine-format`:** `LoggedPlace` gains the dimension, the travel and
`seen_credits`, and its respawn the dimension; an entity's record its portal state; a
commit its travellers. `FORMAT_VERSION` goes to 19 and `STATE_FORMAT` to 25.

**The generator:** `TerrainGenerator::new(seed, dimension)`; the overworld's callers
pass `Dimension::Overworld`, and P5 serves the others.

**The edge:** `WireIds::run(key, count)`, the first of `count` consecutive ids for an
entity of several parts (the dragon's, G 6.4).

### 15.3 What P2 leaves to the steps that use it

- The store's table, with a region's dimension and a home per dimension, and its
  `LevelChunk` keys: P3, which raises `FORMAT_VERSION` again if the table file changes.
  Until then the store says every region is the overworld's, and `homes` names the
  overworld's home alone.
- The portal index's records and query: P3 (the lead's).
- What fills each new field: until its step, every region is the overworld's, no
  place has a travel, no portal clock runs and no fight is kept.

### 15.4 The review of section 15 (2026-10-30), and what P2 builds instead

An independent reviewer read 15.1 to 15.3 against the code at `7bef868`. They found
15.1's argument sound for every message that stays in one dimension: the edge knows
the sender of every entry (`fanout.rs:2088`, merged entries by `came_with`); the
holders named in `RemoteDone`, `NotMine` and `SplitOff` are always of the sender's
dimension; the store's claims, lends, `Elsewhere`, effects, counts and copies are all
the writer's. They found it wrong where a portal fires, and gaps that would reopen P2.
What P2 builds instead:

- **Which coordinates a travelling transfer carries (their D1).** A `PlayerTransfer`
  with a travel, and its riding's vehicle chunk, are in `travel.to`, at `target`: the
  destination's coordinates, which the destination replaces when it places the player.
  The let-go `Has` of the firing tick keeps that place (with its travel), and its
  **face is the source's pose**, filed by the store in the writer's dimension, as are
  the region's own departed faces. A removal reported for the player at the source
  (an edge's reset) uses the source's chunk.
- **The old dimension's watchers (their D2).** The firing region reports the travelling
  player's removal itself, to its edges, as the region that showed them last: the
  destination's `EntityRemoved` reaches only the new dimension's watchers.
- **`Placed` is durable (their D3):** `Durable::Placed`, in the outbox of the player's
  edge, as `Departed` is; a worker passes it on as `PlayerEvent::Placed`.
- **The destination's waiting player (their D4).** A travelling player is taken into the
  destination's state at once, as every arrival is, and waits there:
  `PlayerState.travel: Option<Travel>` until they are placed. A `Has` written while
  they wait (a logout) keeps the travel in its place.
- **The seed (their D5)** is `Region::seed`, the world's seed XOR the dimension's number
  times a constant, used for every stream the region draws, so the overworld's are
  unchanged and the others' apart at every position (built).
- **`RegionEvent::EntityGone`** (their D6) gains `travelled`; it is for entities other
  than players, hence D2.
- **Travellers on the wire and in the log.** `StoreRequest::Commit` and the stream's
  arrivals carry them; a commit's travellers are numbered after its arrivals (index
  `arrivals.len() + i`), always due at once. The log writes untaken travellers forward
  in a record of their own, and a fold at a change of division **never** folds a
  traveller into a chunk: it writes it forward. The store's `KeptArrival` widens to hold
  one (P3).
- **The portal index's query** (3.2 asked for it in P2): `TickOutput.portal_queries`,
  each `PortalQuery { id, centre: BlockPos, radius }` in the region's own dimension, and
  `TickInputs.portal_answers`, each `PortalAnswer { id, portals: Vec<BlockPos> }`, with
  `StoreRequest::Portals` and `StoreReply::Portals` between worker and store. The index's
  records are P3's, which raises `FORMAT_VERSION` to 20.
- **A vehicle with a player** goes in the player's transfer: `PlayerTransfer.unit:
  Option<Unit>`, root first, as a logout takes it (5.1).
- **The cooldown across hand-overs:** `PlayerTransfer.portal: PortalState`, and the
  cooldown in `Place` (`portal_cooldown`), which the game saves.
- **Gateways:** `Way::Gateway { from: BlockPos, exact: bool }`, the source gateway and
  whether its exit is exact, which the game writes back to the gateway. A gateway's
  travel stays in the End, so the edge starts its second half on `Placed` whatever
  the dimension: a placing in the same dimension is a teleport.

### 15.5 The second reading of 15.4

The reviewer found 15.4 answers every finding, and two things more:

- **The vehicle in a transfer (their N1).** The let-go `Has` of the firing tick carries
  the player's unit, as a logout's does, so that a player who leaves before they are
  placed keeps their vehicle in their record. The destination that places the player
  on it says `Took` in the same commit, as the home does when it puts a stay back on
  its unit, so the vehicle is never put back twice. The old dimension's watchers are
  told each entity of the unit is gone, `EntityGone { travelled: true }`.
- **What the wait needs besides the answer (their N2).**
  - `TickOutput.travel_tickets: Vec<ChunkPos>`, the chunks a waiting traveller needs
    loaded, generated or copied (the search's square and the arrival's chunks), which
    the worker keeps as it keeps an arrival's: also the game's portal ticket of 5.2.
  - An answer covers every commit the store has made durable before it answers, the
    asking region's own among them. Answers are not durable: a region asks again after
    a hello, and numbers its queries by its tick and an index, so that no answer to a
    query from before a take-over meets a later one.
  - One radius per query, the traveller's (16 into the Nether, 128 out of it).
- **An edge before P7.** An edge not yet keyed by dimension would take a transfer in
  another dimension's coordinates for the overworld's. Only P9's firing tick makes
  one, and P7 comes first; until P7 the edge's hand-over asserts in debug builds that a
  transfer is the overworld's, so that a P8 test that fires early is caught.

### 15.6 What P2 built (2026-10-30)

As 15.2 to 15.5 say, with these notes:

- **The home chunk.** A region's home chunk is `RegionConfig::home`: the spawn's chunk in
  the overworld, chunk (0, 0) in the others, which a region that holds it never gives
  back, so that a Nether or End home keeps it as 3.4 asks. A region without a spawn
  skips the game's distance to the spawn when it spawns mobs, as the game does outside
  the spawn's dimension (`NaturalSpawner.isRightDistanceToPlayerAndSpawnPoint` compares
  the respawn data's dimension).
- **Placeholders until their step.** The store says every region is the overworld's and
  names the overworld's home alone (P3); it writes a commit's travellers into the log and
  passes a `TravellersForward` over on replay (P3 files them); it does not answer a
  portal query, and logs one (P3). The edge logs a `Durable::Placed` and asserts in
  debug builds that a transfer is the overworld's (P7). The worker passes travellers and
  portal answers into the tick, and travellers and queries on to the store; it keeps no
  chunks for `travel_tickets` yet (P9). The region carries every new field of a player
  through its state, its places, its transfers and its stays.
- **`WireIds::run`** is the edge's own, with nothing on any wire: P12 adds it with the
  dragon, without reopening a contract.
- **Versions.** `FORMAT_VERSION` is 19 (a place's dimension, travel, credits and
  cooldown, a respawn's dimension, an entity's portal state, a commit's travellers and
  their forward record), `STATE_FORMAT` 25.

## 16. P9, the travel, at the level of the code (2026-10-30)

Sections 5.1 and 5.2 say what P9 does; this is how, against the code after P2, P3, P7
and P8 (`fbbb17b` and the edge's change, `44c1845`). P8 left one hook,
`Region::portal_fired(who, kind, entry, output)`, which does nothing; P9 fills it, and
the destination's half. For review before it is built.

### 16.1 What the region needs that P2 did not give it

- **The homes' ids.** A region that fires a player's portal sends them to the
  destination dimension's home (0.6), and a region knows no other region's id.
  `Restored` gains `homes: Vec<(Dimension, RegionId)>`, which the store has, and
  `RegionConfig` gains it from the worker. `STATE_FORMAT` does not change (the config is
  not state); the wire's `Restored` does, before any world exists outside the tests.
- **The travel's target, worked out at the source.** The source knows the source
  portal's block and its rectangle (its own blocks), and computes the travel: the
  destination dimension (the Nether from the overworld, the overworld from the Nether),
  the target `clamp(x × s, y, z × s)` inside the destination's border (G 2.2 item 2),
  and the relative place in the source rectangle (G 2.2 item 6). A rectangle that
  reaches into a chunk the region cannot read is read as far as it can be; the relative
  place then is of what it read (named, rare: a portal across a region line).

### 16.2 The firing tick, at the source (in `portal_fired`)

In the tick a clock fires, after the cooldown is set (P8):

- **A player** (not riding: P8 runs no clock for a rider):
  1. Their transfer is made as at a hand-over (`into_transfer`), then put in the
     destination's coordinates: `dimension` the destination, the pose at the target
     (the block's bottom centre, the look kept), `travel` the travel (15.4).
  2. They leave the region as a departing player does, in this tick, with a
     `Durable::Departed` to the destination's home: the let-go `Has` keeps the place in
     the destination's coordinates with the travel, and its face is the source's pose
     (15.4, D1).
  3. The region reports the player's removal to its edges (`EntityRemoved` at the
     source's chunk), so that no client of the old dimension keeps them at the portal
     (D2). Their own edge has the `Departed` and starts its change (P7).
  4. A vehicle a player rides goes with them (5.1): not until P9b; the clock of a rider
     does not run (P8), so a ridden vehicle's clock firing takes its riders, and that
     waits for the unit's travel below.
- **An entity other than a player**, with its riders that are not players (a unit with
  a player in it is P9b):
  1. Removed from the region with its riders, each reported `EntityGone { travelled:
     true }` with its hops.
  2. Filed: `TickOutput.travellers.push(Traveller { travel, chunk, entities })`, the
     chunk being the target's `LevelChunk`, the entities as the tick left them, root
     first, with their hops raised by one.
- Both are in the tick's commit: the traveller is gone from here and filed there, or
  neither (0.6).

### 16.3 The destination: taking the traveller and the wait

- **A player** arrives at the destination's home by `PlayerChange::Arrive` with a
  transfer whose `travel` is set. `apply_arrival` is as for every arrival: a chunk the
  store said is another's passes them on with `NotMine` (one hop more, the travel
  kept); otherwise they are taken in, the chunk claimed if unknown. Taken in with a
  travel, the player is **waiting**: in the region's state with `travel` set
  (`PlayerState.travel`, durable with the commit), not in the world: no entity is
  shown, their inputs are applied only where `has_no_place` would pass them, and
  nothing moves them.
- **A traveller** comes on the stream (`TickInputs.travellers`, P3), for a chunk the
  region holds. It is taken into a waiting list of the region's, in memory (not
  durable: the store still files it until the commit that takes it).
- **The wait** (5.2 step 1), for each waiting traveller, every tick until it is placed:
  1. The search's square of chunks (radius 16 blocks into the Nether, 128 out of it,
     around the target) is asked to be loaded or copied: `TickOutput.travel_tickets`,
     which the worker keeps as arrival tickets (the arrival region holds them, or claims
     the unheld; another region's come as border copies). Loading a chunk the region
     cannot hold or copy is not waited for: its blocks are taken on the index's word.
  2. One `PortalQuery` per traveller (`QueryId { tick, index }`), asked once; its answer
     comes on the stream behind the commits made before.
  3. When the answer is in and every chunk it waits for is loaded or copied (or a
     limit of 200 ticks has passed, after which it goes on with what it has), it is
     placed, in the order the travellers came; two in one tick see each other's
     portal.
- **A take-over during the wait** restores waiting players from the state (their
  `travel`), and the store files waiting travellers again at the hello: the wait starts
  again (R N5).

### 16.4 Placing (5.2 steps 2 to 6)

1. **Candidates**: the index's answer, and the region's own `nether_portal` blocks within
   the square found in its loaded chunks (its live points of interest: a portal it made
   this tick or since the answer). A candidate in a chunk the region has loaded or
   copied is kept only if its block is still a portal with a horizontal axis; one
   beyond is taken on the index's word.
2. **The choice**: nearest by `distSqr` to the target, then the lowest y (G 2.2 item 3).
3. **Found**: its rectangle (`BlockUtil.getLargestRectangleAround`, 21 by 21, same
   state) from the blocks the region can read.
4. **Not found**: a portal made as `PortalForcer.createPortal` (G 2.3): the spiral over
   the region's loaded and copied chunks; the frame and portal blocks set in the
   region's own chunks, and filed as effects where another region holds the chunk
   (`SetIf`, named in 5.2 step 4). In the overworld or the Nether only; a portal made
   puts its blocks in the same commit as the traveller's placing.
5. **The pose** (G 2.2 item 6): the destination rectangle and the relative place; the
   yaw turned by 90 where the axes differ; velocity kept. `findCollisionFreePosition`
   for an entity of at most 4 × 4, against the region's loaded blocks.
6. **The traveller placed**:
   - a player: their `travel` cleared, pose set, in the world from this tick
     (introduced to watchers as an arrival is), `Durable::Placed { dimension, pose,
     flying }` to their edge after the commit, and the effects they have reported
     (`EffectAdded` for each), as the game resends them; the edge asks the inventory
     (P7);
   - an entity: each of the unit put into the world at the pose (the riders on their
     seats), and the traveller's id among the tick's `arrivals_taken`, so that the store
     drops it in the same commit.
   - If the pose's chunk is another region's (rare: the portal found is beyond a line),
     the traveller is handed on to it as a walk across a line is, placed.
7. **The portal ticket** (radius 3 around the pose, G 2.2): kept by the region as an
   arrival ticket for 300 ticks.

### 16.5 What waits for later steps

- A vehicle ridden by a player through a portal (5.1, R N7): P9b, with the unit in the
  transfer and `Took` at the destination (15.5).
- Zombified piglins from portals (G 4.2): P12.
- The End's portal, the credits and gateways: P10.

### 16.6 Tests

- The region's tests of 16.2 to 16.4, tick by tick: a creative player standing in a
  frame lit in the overworld is let go to the Nether's home in the tick the clock fires,
  with the travel and the place in Nether coordinates; an item thrown in is filed as a
  traveller; the destination finds the index's portal, or makes one, and places them as
  the game would; a second traveller in the same tick uses the first's portal.
- Kill tests at every step of a travel, as N3's: a region killed after the firing's
  commit and before the destination's, the store restarted at every point, the
  destination taken over during the wait: the traveller is placed once, never twice and
  never lost.
- End-to-end: a bot lights a portal in a flat world, walks in, is sent `Respawn` into
  the Nether and placed beside a portal there, walks back, and is placed beside the one
  it came through.

### 16.7 The review of section 16 (2026-10-30), and what P9 builds instead

An independent reviewer read 16.1 to 16.6 against the code at `7bd0b2a`. They found the
firing during an edge's events, its resets and resumes, a logout after the firing, the
source's removal against the edge's hand-over, the edge's hold during the change, and
the numbering of travellers sound; and nine defects. What P9 builds instead:

- **Travellers wait apart from the region's players and entities, durably** (their D1,
  D2, D7, and the list of fifteen places that would have taken a waiting player for a
  present one). A region keeps, in its state, `waiting: BTreeMap<PlayerId,
  WaitingPlayer>` (the transfer and the edge, as `entering` keeps a joining stay) and
  `waiting_travellers: Vec<(EffectId, Traveller)>`. A traveller off the stream is taken
  in the tick it comes, its id among the commit's `arrivals_taken`, so the store files
  it no more and sends it to nobody again; it is the region's state from that commit on.
  None of the region's iterations over players or entities sees them: no physics, no
  faces or stay notes but the waiting one's own, no mobs' view, no pickups, no portal
  clock, no sleep or spawning counts. A split gives the part the waiting travellers and
  players whose target chunk it takes, as it gives arrivals; a merge joins both; a
  region gives back no chunk a traveller waits for (as an arrival ticket keeps one).
  `STATE_FORMAT` rises to 26. A waiting player's inputs are counted applied and passed
  over but for those `has_no_place` passes, which apply when they are placed.
- **The search loads little** (their D3): the chosen candidate's chunks once the
  index has answered, the 3 × 3 chunks of `createPortal`'s spiral around the target,
  and the arrival's chunks; never the whole square. A traveller is **never placed
  without the index's answer**; the limit of 200 ticks applies to loading only, after
  which a chunk not loaded is taken on the index's word or, for the spiral, as
  unsuitable.
- **A returning player with a travel** (their D4, R N4): the overworld's home, given an
  `Entered` whose place has a travel, lets the stay go to that dimension's home (or
  the holder the store names), unplaced, with the travel in the transfer, as it lets a
  stay go to a holder today. The destination takes it into `waiting`, and places it
  with `Spawned` (not `Placed`), since the edge has never put that client into the
  world: its login is then in the right dimension (P7b).
- **The face and the menu at the firing** (their D5, D6): the let-go note's face is
  the source's pose, and no departed face is kept for a portal's departure; the
  player's menu is closed at the firing as at a leave (`menu_leaving`).
- **No vehicle with a player in it fires** until P9b (their D8): its clock runs and
  its cooldown is kept, and it does not fire.
- **No second removal** (their D9): an edge's reset after a firing reports no removal
  for a departure with a travel; the firing reported it, at the source's chunk.
- **Named, rare** (their placing notes): a second traveller in the same tick whose
  portal would be in another region's chunk makes a second portal, as the effects of
  the first are not seen within the tick; and a neighbour update on a region's half of
  a frame made across a line, before the other half's effect lands, breaks the half.

### 16.8 The second reading of 16.7

The reviewer found 16.7 answers D1, D3, D5 to D9, and the rule that nothing is placed
without the index's answer sound; and that a waiting stay kept apart from `players` has
to be met everywhere a stay is. What P9 builds besides:

- **A waiting stay is a stay to every rule that ends or answers one** (their A1, A3):
  the hello's presence answers say `Presence::Entering` for it (the edge waits on that);
  a leave, an edge's reset, a dead stay and an edge gone end it as they end an entering
  one, with a `Has` that keeps the travel (a leave) or a `Gone` (a reset).
- **Reshaping** (their A2): a split gives the part the waiting stays whose target chunk
  it takes, with their edges and in the `SplitOff` list, as it does a player; a merge
  applies its edge resets and its later-stay rule to `waiting` as to `players`.
- **The notes and faces of a waiting stay** (their A4, B2): it writes its `Has`, with
  the travel and no face, when it is taken and in the first tick after a restore; the
  let-go note of a stay that goes with a travel (a firing, or the home letting a
  returning player go) keeps no departed face, and its face is the source's pose at a
  firing, none at the home.
- **Inputs while waiting** (their c): those `has_no_place` passes are kept in the
  waiting stay, durably, and applied in order at the placing; every input of the stay
  is reported applied, and the stay's `last_input` advances.
- **A traveller taken twice** (their A5): a traveller whose id the region holds waiting
  already is passed over; a lend that goes through while a traveller waits is
  survivable (it is placed and handed on, its portal made by effects), and named.
- **The returning player** (their B1, P7's): the edge's `enter_by_arrival`, for a
  transfer with a travel, sends the arrival only, and does not put the client into the
  world: the join's patience runs on, and the destination's `Spawned` puts them into
  the world in its dimension (`tell_entered` takes it from the placing region).

## 17. P11, the rules by dimension, at the level of the code (2026-10-30)

Section 6 says what P11 does; this is how, against the code after P9 (`e2179b7`). A survey
of the simulation found that none of section 6's rules reads the region's dimension yet,
and that some are missing whatever the dimension: a bed's explosion, the respawn anchor
used by hand, ice broken into water, and the sponge. For review before it is built.

### 17.1 The contracts the lead fixes first (one commit, before any family starts)

- **The mobs' and the spawning's view of the level** (`mobs/view.rs` `MobWorld`,
  `spawning/rules.rs`'s level trait, and their implementations in `region/mobs.rs`,
  `region/natural_spawning.rs`, `region/raid_tick.rs` and the tests' views) gain
  `fn dimension(&self) -> Dimension`, so that a mob's rule asks it of the view it already
  has. The region's views answer `self.config.dimension`; the tests' views the overworld.
- **The lava's rules** (`clustine-blocks/src/fluid.rs`): `tick_delay`, `drop_off` and
  `slope_find_distance` take the level's `fast_lava` beside the fluid's kind, every
  caller passing `false` until family B makes the region pass its dimension's. One
  commit changes the signatures and the call sites (`region/fluids.rs`,
  `region/updates.rs`, `region/redstone.rs`, `region.rs`), so that no family edits
  another's lines to do it.
- **Using an item** (`placement::use_item`'s `ItemUse`) gains the level's
  `water_evaporates`, which the bucket reads; the region fills it.
- **The sea level** stays `RegionConfig::sea_level`, set per region; the worker sets it
  by the region's dimension (the overworld 63, the Nether 32, the End 0, as the
  dimensions' noise settings say), not by whether the world has a seed alone.

### 17.2 The families (subagents, each with files of its own)

- **A. Water** (`placement/bucket.rs`, `region/dispensers/behaviours.rs`' emptying,
  `region/random_ticks/melting.rs`, `region/random_ticks/minerals.rs`' dripstone, a new
  `region/ice.rs`): where `water_evaporates`, a water bucket emptied by a player or a
  dispenser makes the fizz (`level_event` 1501 and the sound) and places nothing; ice
  that melts or is broken becomes air (`IceBlock.melt`, `playerDestroy`, which is built
  here for every dimension, with its rule for the block below); dripstone drips no water
  into a cauldron. The sponge is not in Clustine at all; it is named, not built.
- **B. Lava** (`clustine-blocks/src/fluid.rs` and the region's fluid code after 17.1):
  where `fast_lava`, a delay of 10, a drop-off of 1 and a slope distance of 4 (the
  brief, 5.2, inferred; a comparison with the official server settles it), and an
  entity in lava pushed by 0.007 instead of 0.0023333333333333335 (the physics).
- **C. Beds and anchors** (`region/sleep.rs`, a new `region/anchors.rs`,
  `clustine-blocks` for the anchor's charge state where it lacks it): a bed used where
  its rule says it explodes is removed (both halves) and explodes with power 5 and fire,
  hurting nobody until survival (the owner's answer, section 14); `straw_bed_rule` for
  straw beds. The anchor: charged by glowstone in every dimension, with its sound, game
  event and the glowstone used; used empty-handed with a charge, it sets the spawn in the
  Nether (the respawn's dimension the Nether) and explodes elsewhere, with the water
  rule of `RespawnAnchorBlock$1`. Respawning at an anchor (a charge spent) comes with
  the respawn after a death, in the End's milestone.
- **D1. Light and time** (`region/light.rs`, `time.rs`, `region/daylight_detector.rs`):
  a level without a sky (`has_skylight` false) has no sky light (`Lighting::new(false)`
  and `check_light`'s `sky`), and a level with a fixed time darkens never: its
  `sky_darken` is the fixed one, so that what reads it (the endermen's daylight, the
  daylight detector, spawning) follows without its own change.
- **D2. What is off or moved outside the overworld** (`spawning/rules.rs`,
  `region/random_ticks/fire.rs`, `region/raids.rs`, `region/player_effects.rs`' omen,
  `region/custom_spawners.rs`, `mobs/types/snow_golem.rs`, `mobs/ai/navigation.rs`):
  the monster spawn light and the block-light limit by dimension (the Nether: 7 and 15);
  fire's infiniburn tag by dimension (the Nether's and the End's, bedrock in the End);
  no raid begun where `can_start_raid` is false (the omen ends without one); the custom
  spawners in the overworld only; a snow golem melts by `snow_golem_melts` as well as
  by its biome; navigation's heights from the level's, not the overworld's constants.

### 17.3 What P11 leaves

- Piglins' and hoglins' zombification, and zombified piglins from portal blocks: with
  those mobs, in P12.
- A pearl that carries its owner into another dimension (R G10): the lead's, after P12,
  as a change of dimension that no portal made, on P9's travel.
- Compasses, lodestones and clocks: the client's, but `LodestoneTracker`'s clearing of a
  target (the brief, 5.6), which waits for the lodestone itself.

### 17.4 Tests

Each family's rules, tested in both kinds of level, from the brief and the game's code,
by the family; and from the specification by someone else (X), as for every step. The
comparisons with the official server: the Nether's lava's spread and timing (B), and the
anchor's charge and explosion (C), written by a subagent and run by the lead.

### 17.5 The review of section 17 (2026-10-30), and what P11 builds instead

An independent reviewer read 17.1 to 17.4 against the code at `e2179b7` and the dumps.
They found the sea levels, the spawn light by dimension, the infiniburn tags, the raid
gate, the snow golem's attribute, the bucket's order and the bed's sequence right, and
ten defects. What P11 builds instead:

- **Day and night by the game's two tests** (their 1, 2). The game asks
  `isBrightOutside` (`!hasFixedTime && skyDarken < 4`) and `isDarkOutside` (never in a
  level with a fixed time), not the darkening alone; Clustine compares `sky_darken`
  bare in about ten places (endermen, undead burning, drowned, foxes, golems, the
  trader, the villagers' goals, sleep), which in the End (darkening 0) would be day for
  ever and in the Nether (darkening 11) night. The lead's contract commit adds
  `Region::sky_darken()` (`15 − sky_light_level`, by the dimension's clock where it has
  one), `is_bright_outside()` and `is_dark_outside()` to the region and to `MobWorld`
  and the spawning's view, and replaces every bare comparison and every
  `time::sky_darken(self.clock.day)` call with them, in one commit.
- **Fast lava everywhere lava runs** (their 3): `fast_lava()` on `clustine_blocks`'
  `BlockView` and on the physics' `World`, false by default, which `new_liquid`,
  `spread`, `spread_to_sides`, `shape::fluid_delay` (the shape update's tick), the wind
  burst's call and the lava push read; the lead's commit makes the region's views answer
  their dimension's, so that a spread received across a line runs as fast. The values are
  the dump's (`LavaFluid.txt` 317–388: 10, 1, 4), not inferred. Generation keeps the
  overworld's ticks for the structures' fluids (named).
- **Explosions without an entity, with fire and a resistance kept** (their 4): the
  lead's contract gives `Explosion` and the wire's `Blast` an optional source, `fire`,
  `hurts` (false until survival), `decay` and one position whose resistance is
  overridden, before families C and P12's ghast use them.
- **The anchor's and the bed's seams** (their 5): the lead adds the click's arms
  (`placement::click_with`, `redstone::use_by_rule`) to stub functions in
  `region/anchors.rs`, and moves the dispenser's charging there, before family C starts.
- **Sleep by level** (their 6): the night is skipped by the sleepers of the region's own
  dimension only (`WorldPlayer::dimension`): family C.
- **Heights by dimension** (their 7): `lowest_y` is set by the dimension beside
  `sea_level` in the lead's commit; chorus fruit's clamp (`minY + logical_height − 1`),
  `random_pos`'s height and the pistons' top go to D2.
- **Light without a sky kept through reshaping** (their 8): `RegionLight` keeps its
  `sky` through `Clone`, and drops kept sky arrays where there is none: D1.
- **Water** (their 9): dripstone's only dimension rule is mud that does not dry to clay
  where water evaporates (`PointedDripstoneBlock`, 519–545); ice broken by a player turns
  to water only where a player's break makes drops, never in creative, which every
  player is now; the wet sponge is named with the sponge.
- **Contract details** (their 10): `dimension()` on the views has a default (the
  overworld), as `sea_level` has, so that tests' views need no change; a bed's respawn
  record in another dimension is checked again at a respawn, later, not trusted; soul
  fire already holds by `survive.rs` and needs nothing.
- **Tests** (their tests): every rule in all three dimensions, and besides: an enderman
  under the End's sky stays; the night is skipped with a player in the Nether; lava in
  the Nether spreads fast across a region line; a Nether region split or merged has no
  sky light; ice broken in creative leaves air; chorus fruit in the Nether at y 120 stays
  under 128. The comparisons need an oracle scene in the Nether, which a subagent of X
  builds explicitly.

### 17.6 The second reading of 17.5

The reviewer found nine of the ten answers sound, and the explosions' incomplete. What
P11 builds besides:

- **The day's tests have defaults** on the views, from `dimension()` and `sky_darken()`,
  so that the tests' views change for none of it.
- **A `Blast` written to the log reads as before.** Its new flags (fire, hurting,
  decaying drops, no source) take free bits of the flags byte that `put_blast` writes, so
  that an older record reads with all of them as TNT has them. The resistance kept at one
  position needs no field: the exploding region works out every ray itself and files
  effects for the blocks it breaks beyond its line, so the override is the exploding
  region's alone.
- **An explosion without a source entity has a key all the same**, drawn from the
  region's entity ids as an entity's is, so that its drops are merged with no other
  explosion's anywhere (`effects.rs`' rule, 116–120).
- **`hurts` is the bed's and the anchor's, false until survival**; TNT's and every other
  explosion's stay as they are.
- The lead moves the dispenser's charging into `anchors.rs` before family A starts. The
  pistons' top follows from `lowest_y` by dimension and is nobody's to change. D1 changes
  `check_light`'s `sky` with `RegionLight`'s.

## 18. P12, the Nether's mobs, at the level of the code (2026-10-30)

Section 7 lists the mobs; the owner's order (section 14) gives this milestone the
Nether's: the zombified piglin, the magma cube, the ghast and its fireball, the wither
skeleton, the blaze and its small fireballs with the fortress's spawn list, the strider
and its riding, the piglin (its brain, bartering, zombification), the hoglin and the
zoglin, and the piglin brute; with them the two rules P11 left (17.3): piglins' and
hoglins' zombification, and zombified piglins from portal blocks. The End's (the
endermite is built; the shulker, the crystals, the dragon) wait for the End's milestone.
This is how, against the code after P11 (`3738751`). For review before it is built.

### 18.1 What exists

A survey found: a mob type is a `MobType` (`mobs/types.rs`) registered by one module line
and one arm of `built(kind)`, its state opaque bytes in `EntityData::Mob`, so no codec
changes for a mob; the stub pattern (`BUILT = false`, a guarded arm) of N9f-3; the
Nether's biome spawn lists in the generated data, the fortress's structure list in
`natural_spawning.rs`, and the spawn rules of the blaze and the zoglin; the piglins' and
hoglins' brain memories declared; a pig's steering by an item; items thrown by a mob
(`Local::Throw`). Missing: any fireball (a new kind of entity, not a mob), the biomes'
spawn costs (soul sand valley, warped forest), the fortress's list asked on nether bricks
only, a mob that stands on a fluid, and the spawn rules of the rest.

### 18.2 The contracts the lead fixes first (one commit)

- **Stubs** for the ten types, each `BUILT = false` with its arm in `built(kind)`, and their
  entries in the shared tables (`body/ladder.rs`' families and sound sources,
  `ai/sensing.rs`' monsters, `region/mobs.rs`' lists), so that no family edits a table.
- **Spawn rules** by type: `spawning/rules.rs` calls a function in each family's file for
  its kind (`check_spawn_rules`, false while the stub is unbuilt).
- **Fireballs**: `EntityData::Fireball { kind (large, small), owner, acceleration,
  explosion_power, deflected, life }` with its codec (`clustine-world`, `clustine-format`),
  a stub `region/fireballs.rs` with `Region::shoot_fireball(...)` and its tick and hit,
  and the edges' showing of it (the protocol's spawn packet and its data).
- **The fortress's spawn list on nether bricks** (`NaturalSpawner`'s structure override,
  the brief's 5.10) and the **spawn costs** of the Nether's biomes
  (`PotentialCalculator`), in `natural_spawning.rs`, the lead's.
- **Seams**: steering by an item generalised from the carrot on a stick to the warped
  fungus on a stick (`region/riders.rs`); `MobType::can_stand_on_fluid` read by the
  pathfinder; a portal block's random tick calling the zombified piglin's spawn
  (`portals.rs`, P8's file), a stub in the family's file.

### 18.3 The families (subagents, files of their own)

- **E1. Zombified piglin and magma cube**, with the portal spawn (the brief's 4.2 and
  5.8) and the magma cube on `cube_mob.rs`' rules.
- **E2. Fireballs and the ghast**: `region/fireballs.rs` (flight, acceleration,
  deflection by a hit, impact: the large one's explosion with fire, the small one's fire
  and damage), and the ghast (flight, its shooting goal, its sounds and level events).
- **E3. Blaze and wither skeleton**: the blaze's goal shooting small fireballs through
  the contract's call, its hovering and its water hurt; the wither skeleton's sword,
  wither and flaming arrows.
- **E4. Strider**: lava walking (`can_stand_on_fluid`, the fluid collision), warm and
  cold, steering with the warped fungus on a stick, its saddle, its spawn in lava with a
  rider by chance.
- **E5. Piglin and piglin brute**: the brain, bartering (`gameplay/piglin_bartering`,
  admiring, throwing), gold armour, anger at gold taken or mined, zombification.
- **E6. Hoglin and zoglin**: the hoglin's brain, its avoidance of warped fungi, portals
  and respawn anchors, breeding with crimson fungi, its conversion to a zoglin; the
  zoglin.

### 18.4 Tests

Each family's rules from the brief and the dumps, by the family; from the specification
by someone else (X); comparisons with the official server of a ghast's fireball, a
blaze's volley, piglin bartering's draws and a strider on lava, in a Nether scene, which
an X subagent builds.

### 18.5 The review of section 18 (2026-10-30), and what P12 builds instead

An independent reviewer read 18.1 to 18.4 against the code at `3738751` and the dumps.
They found the stubs, the tables named, the fortress list, the spawn costs, the steering
seam, the ghast's timings and the fireball's griefing right, and twelve defects. What
P12 builds instead; the contract commit of 18.2 grows by all of the lead's items:

- **The fireball as the game keeps it** (their 1): `acceleration_power` (0.1, the
  motion `(m + m̂·p)·0.95`, 0.8 in water), the owner as a `Thrower` (key, type, a
  player's UUID), `left_owner`, `has_been_shot`, its item (a fire charge by default); no
  `life` and no flag for a deflection, which the ghast reads from the cause being a
  player. A fireball whose owner is not the region's is handed across a line as a
  `Thrown` is, and is discarded only when its owner is the region's own and removed.
- **Deflection** (their 2): `EntityUse::Attack` carries the attacker's look (with its
  codec), so that a fireball held across a line is deflected there; the fireball's arms
  in the shared code (`entities.rs`' uses and ticks, `explosions.rs`, `thrown.rs`,
  `projectiles.rs`' pickable, the edge's `owner_of` and `spawn_packets`) call into
  `fireballs.rs`; a deflection syncs its velocity at once.
- **Mobs shoot through `Local`** (their 3): a fireball `Shot` in the contract; E3 is
  briefed after E2 is merged.
- **Hits with extras** (their 4): `HitExtra::Throw` and a type's hook that fills
  `HurtBy.effects` (the wither skeleton's wither), in the contract.
- **Conversions** (their 5): `Local::Convert` takes a level event or a sound, and an
  effect given after (nausea for 200 ticks), in the contract.
- **The brain's tables** (their 6): the piglins' and hoglins' sensed memories in
  `MemoryKind::sensed()`, and the accessors each family reads of the other's mob (a
  hoglin's `CannotBeHunted` and age, the adult piglins counted), declared in the
  contract with stub bodies.
- **Piglins' anger** (their 7): the calls from breaking a `#guarded_by_piglins` block and
  opening a guarded container, and an `EntityUse` that angers a piglin across a line,
  with the broadcast of anger and the hoglins' group attack through it, in the contract.
- **The spawning's tables** (their 8): the walk target values of the strider (10 on
  lava) and the hoglin (0, 10 on crimson nylium), the strider's obstruction rule, the
  ghast's cluster size of 1, the hoglin counted by its class (an animal), and the
  Nether's ambient light (0.1) in the light the spawner reads, all the lead's.
- **Standing on lava** (their 9): the physics' `Context` knows whether the entity stands
  on fluids and the state above a block, so that source lava under a strider is a half
  block where the game's is; the travel's gate; and a strider navigation (stable on lava,
  its maluses, its start node), all the lead's; E4 builds the strider on them.
- **The structures' mobs** (their 10): a bastion's piglins and brutes and the striders
  made with chunks belong to E5 and E4, and `ENTITIES_GENERATION` rises after them;
  spawner blocks spawn nothing yet, so the fortress's blaze spawners are idle, named.
- **What a player meets at once** (their 11): piglins' crossbows (E5); a fireball lights
  TNT and campfires it hits (E2); a large fireball's direct hit reaches a target across a
  line before its blast (E2); the strider jockey after E1 (E4 is briefed after E1).
- **Tests** (their 12): bartering by its table's outcomes, the admiring's 119 ticks and
  the throw's sequence, not by the game's draws; and on a divided world: a ghast's
  fireball across a line, a deflection across a line that kills the ghast, piglins
  angered across a line by gold mined, striders, hoglins and ghasts spawning as they
  should, a strider over a line on lava, a bastion's piglins and brutes, a small
  fireball's fire across a line, conversions keeping equipment with nausea and their
  sound, and zombification in the overworld only.

### 18.6 The second reading of 18.5

The reviewer found 18.5 answers all twelve, nine of them soundly as written. What P12
builds besides:

- **The fireball's end**: discarded also when its owner died in this region while the
  fireball is here, and when it reaches a chunk no region holds (the game's
  `hasChunkAt`), so that a fireball that misses does not wait in unloaded land.
- **A deflection names its player**: `EntityUse::Attack` carries the attacker's UUID
  beside the look, so that a ghast killed across a line by a deflected fireball is killed
  by that player (`last_hurt_by_player`), with its tear and experience.
- **A hoglin's throw lands on a player**: `HitExtra::Throw` on a player's hurt path (a
  motion sent to their client) is the lead's in the contract; the lead checks that a
  player takes `HurtBy.effects`, for the wither skeleton's wither.
- **Anger told to a mob's type**: the contract's arm for the anger across a line calls
  a stub `MobType::told(&Told)` (the attacker's key and type, whether the anger is
  universal), which E5 and E6 each fill for their own mobs; the receiving region judges
  its piglin's sight of the player where `onlyIfSeen` asks it (a container opened).
- **Light and generation**: the ambient light reaches `magic_value` by the dimension,
  as chunk generation's spawning reads it too.
- **Standing on lava** also in the check of a ridden vehicle's move and the navigation's
  `clear_between`, each of which builds its own `Context`.
- **The bastion's hoglins** belong to E6 beside E5's piglins and brutes; a template's
  entities are made where their type is built (`is_built`), so no family touches that
  code; `ENTITIES_GENERATION` rises in one lead commit after the three families.
- **Tests** besides: a deflection by a player in another region than the fireball,
  with the ghast's tear and experience; a hoglin throwing a player.

## 19. The End, at the level of the code (2026-10-31)

The owner's second milestone of P (section 14): the End's portal, platform, credits and
gateways (P10), the End's land, its mobs and the dragon's first fight (P12's End part),
and the void that kills, with the respawn it needs (question 7). This is how, against
the code at `9bd8709`. For review before it is built.

### 19.1 What exists

A survey found: P2's contracts (`Way::{EndPortal, Gateway { from, exact }, Credits}`,
`PortalKind::{End, Gateway}`, `DragonFight` and `RegionState.dragon_fight`,
`PlayerState.{seen_credits, in_credits}`, the End's home region made with the world),
encoded and carried but used nowhere; the End's terrain and biomes, served bare
(`bin/clustine/src/lib.rs`, `bare_land`); the End city's starts and pieces, not placed;
the stronghold's portal room with its frames and eyes; the chorus flower's growth and the
chorus fruit's teleport; the raid's boss bar at the edge (`boss_bars.rs`). Missing: the
End's features; using an eye on a frame and throwing one; the End portal block's travel;
the platform; the credits; gateways; a player's health, death and respawn; the shulker,
its bullet, the end crystal, the dragon and its fight; an entity of several parts on the
wire (`WireIds::run`). P9's travel assumes a Nether portal in `portals.rs` (`portal_met`,
`entity_portal_inside`, `players_portal_tick`), `travel.rs` (`travel_fired` returns for
any other kind) and `arriving.rs` (every waiter searches the portal index; `choose`
keeps Nether portal blocks); the edge's change of dimension is wired for a change of
level only, so a gateway takes a plain hand-over's path.

### 19.2 The steps, in order

**P10-0. The contracts, the lead's, one commit.** Everything the steps below share, so that
no step reopens a type:
- *Entities:* `EntityData::EyeOfEnder { target, life, survives, item }`,
  `EntityData::EndCrystal { beam_target: Option<BlockPos>, show_bottom }`,
  `EntityData::ShulkerBullet { owner, target, steps, direction, target_delta }` and the
  dragon's fireball as a `Fireball` kind (P12's), with their codecs; stubs `BUILT = false`
  for the shulker and the dragon with their arms in the shared tables, as 18.2 did.
- *A player's life:* `PlayerState` gains `health` (20 at a join) and `dead` (the respawn
  screen shown); `PlayerInput::PerformRespawn` (the client command 0) and
  `PlayerEvent::{Health, Died { message }, WonGame { seen }}`; `Way::Respawn` beside
  `Way::Credits`, for the respawn after a death, which carries nothing but where it was
  asked from. The edge's packets: `SetHealth`, `PlayerCombatKill`,
  `GameEvent(WIN_GAME)`, `GameEvent(IMMEDIATE_RESPAWN)` where the game sends it, and the
  decoding of `ClientCommand`.
- *The strongholds:* the positions of the 128 strongholds of the concentric rings are a
  function of the seed and the overworld's biome source; the world store works them out
  once when the world is made, keeps them in `meta`, and gives them to every region in
  its configuration, so that an eye thrown anywhere finds the nearest without a question
  across the cluster (`findNearestMapStructure` with radius 100 reads only those).
- *Several parts on the wire:* `WireIds::run(key, n)`, n consecutive wire ids for one
  entity, given back together; the dragon's eight.
- *The fight's place:* the End's home region holds the arena, the chunks from −8 to 8 on
  both axes (the game's ticket of radius 9 covers them), as a pinned area of its own
  that no split takes from it and that is never merged away from the home
  (`Region::split` and the coordinator's policy both read it).

**P10a. The End's land** (subagents, files of their own; the comparisons of F and S).
The End's features as the game's are placed (`end_spike`, with its crystals made as
entities of the chunk and its iron bars where caged; `end_island`; `chorus_plant`;
`end_gateway_return` in the highlands; `end_platform` at (100, 49, 0)); the End served
with its features (`featured_land`); End cities placed with their markers: loot chests,
sentry shulkers made where their type is built, the elytra in its item frame on the ship.
`ENTITIES_GENERATION` and the features' generation rise once, after P10a and the
shulker, in one lead commit. Compared with the official server's chunks of a seed, as
the overworld's and the Nether's were.

**P10b. The portal and the eyes** (a subagent for the eye and the frame; the travel the
lead's). `EnderEyeItem.useOn` and the frame's eye, the 5 × 5 ring found, the 3 × 3 of
`end_portal` made, level events 1503 and 1038 (global); the thrown eye (`EyeOfEnder`'s
flight, 80 ticks, a draw of four in five, `ender_eye_death` or level event 2003, its
item dropped). The End portal block: `PortalKind::End`, transition 0; into the End the
traveller goes to the End's home with `Way::EndPortal`, and the region that holds
(100, 49, 0) makes the platform (`createEndPlatform`, its blocks destroyed with drops
first) in the commit that places them: a player at (100.5, 49, 0.5), any other entity at
(100.5, 50, 0.5), yaw 90, the relatives of 2.4. No portal search and no index for the End.
Out of the End: a player who has not seen the credits gets the credits (P10c); one who
has, and any entity, goes to the overworld: a player to their bed or anchor without
spending a charge, else the world spawn (5.3, R G8), any other entity to the top of the
world spawn's column. The edge's placing tells the portal's sound (1032) where the
game's transition plays it, and the default spawn of the level the client is in.

**P10c. The credits** (the lead's): as 5.3 steps 1 to 4. `GameEvent(WIN_GAME, 0)`; the
player hidden and still in the End's region, inputs but `PerformRespawn` ignored; then a
departure with `Way::Credits` and `data_to_keep` 1 to where the respawn puts them.

**P10d. Death and the respawn** (the lead's). Only the void kills: below the End's (and
any level's) lowest y less 64, `fell_out_of_world` hurts a player by 4 a tick, creative
or not (`bypasses_invulnerability`); health is sent; at 0 the player dies: the death
message (`PlayerCombatKill`), the respawn screen, nothing dropped (every player is
creative and the game's creative player drops what it carries — **to be checked in the
dump before it is built**), the stay kept in the region, dead, until `PerformRespawn`;
then a departure with `Way::Respawn` and `data_to_keep` 0 to the bed, the anchor (its
charge spent) or the world spawn, `NO_RESPAWN_BLOCK_AVAILABLE` where the block is gone. A
dead player who leaves enters next time at their respawn. Health comes back full at the
respawn; nothing else hurts a player yet.

**P10e. Gateways** (the gateway's block entity a subagent's; the travel the lead's). The
block entity's age, cooldown and beam (block event 1), its exit found or made 1024 blocks
out (the store's test for an empty chunk; the island made by the region that holds the
exit, with `end_island` of the position's random), the return gateway placed; the travel
a hand-over within the End with `Way::Gateway`, which the edge treats as the second half
of a change of dimension does: moves held, the placing a teleport the client must
accept (15.4's "on `Placed` whatever the dimension").

**P12-End. The End's mobs** (subagents by family, as 18.3; the fight's record the
lead's):
- *E7. End crystal:* placed by its item on obsidian or bedrock where the two above are
  air and empty; explodes (power 6) when hurt but by an explosion that is not its own,
  removed at once; its beam; it fires the fight's respawn check where it is placed.
- *E8. Shulker and its bullet:* attached to a face, its peek and its armour, teleporting
  when hurt at low health, the bullet's flight by steps and levitation 200 ticks on a hit,
  the duplication when a bullet hits a shulker, its colour; the sentries of the cities.
- *E9. The dragon* (a subagent, against the contract): its eight parts and their hits,
  its phases (holding pattern, strafe, approach and landing, sitting, take-off, charge,
  death) as the dumps have them, its fireball and the dragon's breath cloud, the crystals'
  healing, its death's experience (12000, then 500).
- *The fight* (the lead's): `EnderDragonFight` ticked by the End's home region: the
  scanning for an old portal, the dragon made, the boss bar shown to players within the
  arena, the crystals counted, the death: the exit portal (`end_podium`), the egg where
  it was never killed before, the next of twenty gateways. Its respawn by four crystals
  last, and named if it slips (section 14).

### 19.3 Tests and comparisons

Each family's own tests; tests from the specification by someone who did not write the
code, for P10b to P10e together and for the End's mobs; comparisons with the official
server on a world of one seed on both (`OracleWorld` with the seed, so that the End is
the same): an End chunk's features, the platform and the arrival's packets, the
exit portal's credits (the `WIN_GAME` and the respawn's packets), an eye's flight and
where it goes, a crystal's explosion, the void's death and respawn, and the dragon's first
moves (its spawn, the boss bar, the first nodes of its path).

### 19.4 What is left out, named

The wither (section 14); the dragon's respawn may slip past the milestone if it comes
last; survival's other harms (a player hurt by anything but the void); a creative
player's drops at death if the dump says the game drops them (then they are dropped
where they fell, which in the void is nowhere).
