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
