# Vanilla parity matrix

Tracks how closely Clustine matches the vanilla server. This is the main place to find
something to work on. Brought up to date on 2026-11-02, row by row against the code and
the tests at commit `97c3bc4` (phases N and P done), with the evidence in each row.

Target Minecraft version: **26.3** (protocol 777).

## In short

Every player is in **creative mode**: there is no survival mode, so no damage to
players from anything but the void, no hunger, no breaking times, tools or drops from a
player's break, and no crafting or smelting. Besides that, what a player meets is the
game's, and most of it is compared with the official server: the overworld, the Nether
and the End as the game generates them, blocks placed and used and updated by its
rules, light, fluids, redstone, the animals, monsters and villagers with their AI,
spawning, trading and raids, vehicles, projectiles and explosions, the portals, the
dragon fight, the gateways and the credits.

What is missing, by what a player would notice first:

1. **Survival mode**: health, hunger and damage to players; breaking times, tools and
   drops; crafting, smelting and the menus of the blocks that work items (furnaces,
   brewing stands, the crafting table, anvils, enchanting tables and the rest);
   enchantments' effects on weapons, tools and armour.
2. **Commands and chat**: players cannot chat or use commands.
3. **What other players see of a player**: poses (sneaking, swimming, gliding), skin
   layers, the main hand; the player list's latency.
4. **Things that are inert**: trial spawners, signs' text, jukeboxes, cauldrons, sculk
   shriekers and catalysts, crafters, structures' chests (their loot is not rolled),
   bubble columns forming.
5. **Weather**: it never rains, snows or thunders.
6. **Mobs not built**: the breeze, the creaking, the warden, the wither, the
   illusioner, the giant and the sulfur cube.
7. **Online mode**: offline login only, no encryption.

The order of the work after phase P is in the roadmap's first section: what phase P
left named, then survival with the rest of this matrix. Survival is planned in
[v-plan.md](groundwork/v-plan.md) (drafted and reviewed once on 2026-11-02, steps V1 to V13, to be read again
and agreed with the owner before it is built).

## Status values

| Status | Meaning |
|---|---|
| not started | No implementation |
| partial | Implemented, known differences from vanilla (link the issues) |
| implemented | Believed complete, not yet covered by a differential test |
| verified | Covered by a differential test against the vanilla server, or against the game's own classes run from the pinned jar, that passes |

## The differential tests

All of them are ignored by default: they need Java and the server jar that `cargo
datagen` downloads, a world the official server made, or both. Each file's head says
how to run it; `tools/oracles.sh` runs the comparisons with the official server one
target at a time under a limit of its own. As of 2026-11-02 every one of them passes
on its last recorded run but `structure_entities_oracle.rs`, last recorded failing on
2026-10-29 for a reason the roadmap gives that has since gone (it waited for S3): to
be run again.

Against the official server (`bin/clustine/tests/`):

- `oracle.rs`: joining, walking, seeing other players, breaking and placing a block,
  a second login, entering again after leaving in flight, light after blocks change.
  `join.rs` holds Clustine to what the second login and the flight showed.
- `placing_oracle.rs`: 243 cases of placing (90), attaching (88), using (24), breaking
  (24), flowing (5), buckets and lily pads (12), compared block state by block state.
- `redstone_oracle.rs`: eleven circuits, packet by packet and tick by tick, also
  built across a pinned line.
- `containers_oracle.rs` (a chest's window), `merchant_oracle.rs` (the merchant
  window), `components_oracle.rs` (stacks with components on the wire and their click
  hashes).
- `entities_oracle.rs`, `items_oracle.rs`, `tnt_oracle.rs`, `minecarts_oracle.rs`,
  `projectiles_oracle.rs`, `effects_oracle.rs`, `golems_oracle.rs`, `mobs_oracle.rs`,
  `monsters_oracle.rs`, `chunk_animals_oracle.rs`, `structure_entities_oracle.rs`,
  `spawning_oracle.rs` (a day of natural spawning), `growth_oracle.rs` (growth by
  light), `detector_oracle.rs` (daylight detectors through a day),
  `villagers_oracle.rs`, `raids_oracle.rs`, `nether_mobs_oracle.rs`.
- `dimensions_oracle.rs` (the Nether: a change of dimension, lava, water, beds and
  anchors), `end_oracle.rs` (into the End, the platform, the bar, the void's death and
  the respawn), `dragon_oracle.rs` (the dragon's first thirty seconds),
  `dragon_death_oracle.rs` (its death, what it leaves, the credits),
  `gateways_oracle.rs` (a gateway's travel and its exit).
- `sculk.rs` (a vibration) and `swings.rs` (an arm's swing) each hold one comparison.
- `services/edge/src/encode.rs`: a flat chunk's packet.

Against the game's own classes and data:

- `crates/clustine-light/tests/official.rs`: the light of chunks the official server
  made and lit.
- `crates/clustine-terrain/tests/official.rs`: the biomes of chunks the official
  server made.
- The Java judges in `crates/clustine-terrain/tests/java` (`GameBiomes`, `GameFill`,
  `GameSurface`, `GameCarvers`) and `crates/clustine-structures/tests`
  (`GameStructures`, the bent ground) have the pinned jar's generator do one step for a
  list of chunks; `services/worldgen/tests/game_features.rs` runs the official server
  in Java to place features and structures area by area, in all three dimensions.
- `crates/clustine-sim/tests/paths_official_server.rs`: the jar's own path finder on
  521 layouts.
- `crates/clustine-terrain/tests/game_spawn.rs` and
  `services/worldgen/tests/official_spawn.rs`: a new world's spawn.

## T0 — Protocol and session

| Feature | Status | Notes |
|---|---|---|
| Handshake, status, login, configuration | partial | Offline mode only. What a joining client is told (registries, tags, join data, the difficulty) is verified by `join_matches_the_official_server`; a second login puts the first out with the official server's reason (`join.rs`). Not built: online mode and authentication, refusing a login when the server is full (`max_players` is only advertised), the status's favicon and player sample, keep-alive during configuration, cookies, transfers and server links |
| Encryption and compression | partial | Compression implemented (`--compression-threshold`), with `join.rs` tests with and without it. Encryption, which online mode needs, not built |
| Chunk and light data streaming | partial | Flat chunks verified (`flat_chunk_matches_the_official_server`), the area sent about a player (`walking_matches_the_official_server`), light after changes (`light_after_blocks_change_on_the_official_server`), End chunks through the protocol (`end_oracle.rs`' platform, `gateways_oracle.rs`' exit). Overworld and Nether chunks of a seed are judged in the crates (T2), not through the protocol. Pacing differs: batches go out on the client's acknowledgement (9 first, at most 64, one outstanding), not per tick with the game's window of 10 |
| Where a new player enters | partial | The world's spawn as the official server chooses it (T8); a player stands on the spawn column; one who left comes back where they left, in any dimension, looking the same way and flying if they flew. Missing: the spread of joins and respawns over the spawn radius, which the official server does in creative too (`respawning.rs`, named); `/setworldspawn` |
| Player movement | partial | Positions are taken from the client, clamped to the world's limits: no speed or collision checks, several positions a tick, moves before "player loaded" taken. Moves before a teleport is confirmed are dropped. A sleeper's and a rider's moves follow the game; a vehicle's move is checked by the game's rules (`physics/vehicle_move.rs`) |
| Seeing other players | partial | Appearing and leaving verified (`players_appear_like_on_the_official_server`); the arm's swing verified (`swings.rs`); equipment sent when it changes and when a player comes into view; crouching, sprinting and effect particles in the entity's data; the sleeping pose. Not sent: the poses `CROUCHING`, `SWIMMING` and `FALL_FLYING`, skin layers and the main hand. Moves are sent as absolute positions with each move, not as the game's deltas |
| Player list | partial | Names, creative mode; a player on another edge is listed while their entity is shown. Missing: latency, display names, header and footer, skins (online mode) |
| Keep-alive and timeouts | implemented | Every 15 seconds in play; an unanswered one disconnects ("Timed out"), a wrong id is a protocol error (`join.rs`) |
| Chat and commands | not started | The server sends system messages (sleep, anchors, the death message, verified in `end_oracle.rs`); a player's chat and commands are dropped, no command tree is sent, no command exists |
| Game modes | partial | Creative only, for everyone; no change of game mode. Planned: each player's own mode, the server's default by `--gamemode` (`groundwork/v-plan.md`, V1) |
| Health, food, experience | partial | Health and experience told; food and saturation stay full. Health compared in the void's death (`end_oracle.rs`). Named: a creative player's health comes back between the void's hits, and a health of 0 is told after the death screen |
| Death and respawn | verified | Only the void kills (creative); the respawn at a bed, an anchor or the world spawn, with the last death's place (`end_oracle.rs`, `dragon_death_oracle.rs`). Named: no spread over `respawn_radius` |
| Change of dimension | verified | Nether portal, End portal, gateways, credits (`dimensions_oracle.rs`, `end_oracle.rs`, `gateways_oracle.rs`, `dragon_death_oracle.rs`). Named: not sent are the permission level, the command tree, a console line, the world border, the tick rate and post effects |
| Boss bars | verified | Raids and the dragon (`raids_oracle.rs`, `end_oracle.rs`, `dragon_death_oracle.rs`). Named: the bar's removal comes after the dragon's within their tick |
| Time of day | verified | One packet a second with the clocks; sleep skips the night; the clocks of each dimension (`mobs_oracle.rs`' time, `detector_oracle.rs`) |
| Difficulty | verified | Easy, unlocked, not changeable by a client (`join_matches_the_official_server`) |
| Sounds and particles | partial | Compared scene by scene in the comparisons above; no general comparison |
| Titles, world border | not started | |

## T1 — Blocks, items, inventory

| Feature | Status | Notes |
|---|---|---|
| Block breaking | partial | Creative's instant break, for the digger, bystanders and latecomers (`breaking_a_block_matches_the_official_server`, `placing_oracle.rs`' 24 breaking cases): two-block doors, beds and plants go whole, what rested on the block goes with it, containers spill. Blocks broken by rules (updates, water, decay, pistons, explosions) drop by their loot tables. Not built: survival breaking (progress, times, tools, the right tool, hardness), drops from a player's own break, tool wear, ores' experience, silk touch, fortune and shears in loot, loot that copies components or state (a shulker box, a banner drop plain) |
| Block placing | partial | Vanilla's placement rules, verified by `placing_oracle.rs`' 243 cases. Used by a click: doors, trapdoors, gates, pots, beds, anchors, levers, buttons, repeaters, comparators, daylight detectors, note blocks, bells, wire, and every container below. Crops and mushrooms need their light. Not built: signs' editor and text; the blocks that work items (below); the classes whose survival rule is not ported, which place nothing (cake, vine, cocoa, amethyst buds, chorus, dirt path, hanging roots, pale hanging moss, shelf mushroom, spore blossom, pale moss carpet); a player's tools on blocks (stripping, tilling, paths, waxing, scraping); block entity data copied from the item (banner patterns, shulker box contents, names); a music disc into a jukebox |
| Block state and neighbour updates | partial | Phase B: updates depth first as `Level.setBlock`, shapes, what rests on what, scheduled ticks durable across merges and crashes, falling blocks, post-processing, seams between regions; rails with their curves by power (R3b); random ticks. Verified by `placing_oracle.rs` and `redstone_oracle.rs`. Missing: what the rows below name |
| Inventory | partial | All 41 slots (hotbar, main, armour, off hand, F), the ender chest, stacks with their components, kept across leaving. Missing: the player's own inventory screen driven by the server, 2×2 crafting, pick-block from a block, equipping armour from the hand, renaming |
| Containers | partial | Menus with the game's clicks and synchroniser: chests (all kinds, double), barrels, the ender chest, shulker boxes, dispensers, droppers, hoppers, chest boats and carts, the saddle menus, the merchant. Verified: a chest's window (`containers_oracle.rs`), the merchant (`merchant_oracle.rs`). No menu: furnace, smoker, blast furnace, brewing stand, crafting table, anvil, enchanting table, grindstone, stonecutter, loom, smithing and cartography tables, beacon, crafter, lectern. Named: a container a step across a pinned line opens only from its own side, and two chests across a line stay single |
| Crafting, smelting, other recipes | not started | No recipe code, no recipe packets, no furnace or campfire cooking. Trading is built (T3) |
| Item components | partial | 17 kinds kept, on the wire and hashed as the game hashes them (verified, `components_oracle.rs`): potions, enchantments, stored enchantments, charged projectiles, fireworks, damage, banner patterns, item name, rarity, dyed colour and others. Others are dropped from a stack: custom name, lore, container contents, block entity data, written books, heads' profiles, custom data, goat horns' instruments |
| Enchantments | partial | Effects built: Power, Punch, Flame, Infinity, Multishot, Piercing, Quick Charge, Loyalty, Impaling, Unbreaking, Mending; random enchanting for trades and mobs' equipment. No effect: Sharpness, Smite, Bane of Arthropods, the Protections, Feather Falling, Thorns, Fire Aspect, Knockback, Looting, Efficiency, Silk Touch, Fortune, Respiration, Aqua Affinity, Depth Strider, Frost Walker, Soul Speed, Swift Sneak, Sweeping Edge, the curses, Luck of the Sea, Lure, Density, Breach, Wind Burst, Riptide, Channeling. No anvil, enchanting table or grindstone |
| Fluids | partial | `FlowingFluid`'s rules, verified (`placing_oracle.rs`' flowing cases); what water washes away drops; lava fast and water evaporating in the Nether (verified, `dimensions_oracle.rs`); campfires and candles put out. Missing: bubble columns forming (existing ones push boats); cauldrons |
| Lighting | verified | Light made with a chunk equal to the official server's (`crates/clustine-light/tests/official.rs`); light after changes (`oracle.rs`); light in the simulation (N8), read by growth, detectors and burning, compared with the official server (`growth_oracle.rs`, `detector_oracle.rs`, `monsters_oracle.rs`) |
| Beds | partial | Sleep skips the night, the respawn point, beds explode in the Nether and the End (verified, `dimensions_oracle.rs`). A player's sleep not compared |
| Respawn anchors | verified | Charged, set the spawn in the Nether, explode elsewhere (`dimensions_oracle.rs`) |
| Note blocks, bells | implemented | Tuned, played, rung by hand and by redstone; villagers' meeting and alarm. Not compared alone |
| Sculk sensors | verified | Plain and calibrated (`sculk.rs`). Named: not heard across a region line |
| Item frames, armour stands, paintings, leads | implemented | Not compared. Named: taking from an armour stand across a line does nothing, a lead across a pinned line snaps |
| Signs, banners | partial | Placed in the game's states (verified). No text or editor; banners placed without their patterns; no loom |
| Bees and beehives, composters, campfires, lecterns | partial | Bees, honey, farmers' composting, campfires lit and burning, lecterns as job sites. A player's use of a hive, a composter or a lectern does nothing; campfires do not cook |
| Jukeboxes, cauldrons, sculk shriekers and catalysts, crafters | not started | |
| Pressure plates and tripwire | implemented | Pressed by players, items, carts, boats, falling blocks, TNT, armour stands and mobs (a bat and a marker stand ignore them; stone plates count only living things); mobs light redstone ore, a crouching cat or ocelot does not (`mobs_press.rs`, `mobs_on_plates.rs`). A mob on a plate compared with the official server in `mobs_on_plates.rs` (ignored; not yet run). Named: a mob's or an item's walk into an unpressed plate of another region's land does not press it until it is in that region |

## T2 — World generation

Everything here is in the crates `clustine-terrain`, `clustine-features` and
`clustine-structures`. Every world has its three dimensions: a flat overworld whose
Nether and End are seed 0's, or with `--seed` the game's three. Structures built of
templates (villages, bastions, ruined portals, shipwrecks, igloos, ocean ruins,
mansions, buried treasure, Nether fossils, End cities, fossils, desert wells) need the
game's jar (`--minecraft-jar`, or the one `cargo datagen` keeps); without it they are
not placed. A flat world reads it for its Nether and its End.

| Feature | Status | Notes |
|---|---|---|
| Biomes | verified | Equal to chunks the official server made on seed 13579 but four exact ties, and to the game's classes in all three dimensions (`GameBiomes`) |
| Terrain (fill, aquifers, surface, veins, bent ground) | verified | Equal to the game's classes block for block in all three dimensions, on several seeds (`GameFill`, `GameSurface`, `GameStructures`' bent ground). Not done: F8, finished chunks against the official server's saved ones |
| Carvers | verified | Caves and ravines, in all three dimensions (`GameCarvers`) |
| Features | verified | Placed in ADR-0021's order and equal to the official server's chunk for chunk (`game_features.rs`): the overworld's over an area of each of its 56 biomes on two seeds; the Nether's (seed 13579, 144 chunks, with a fortress and fossils); the End's arena, platform, outer islands and a city (seed 13579). Named: a surface block at an exact biome tie may follow the other biome; icebergs differ where HotSpot's `Math.sin` and `Math.cos` differ from fdlibm, one block in 204 areas and 571 in another |
| Structures | verified | Which start where with every piece, all 52 kinds over 58,864 chunks (`GameStructures`); their buildings with the features. Bastions' buildings are not shown to be in a judged area. Chests get the game's loot table and seed, but are not filled when opened (T3) |
| Entities made with chunks | partial | Animals as the game makes them, verified on two seeds (`chunk_animals_oracle.rs`; named: they have ticked before they are shown); striders in the Nether, not compared; structures' mobs (villages, huts, monuments, outposts, bastions, cities' shulkers, the ship's elytra) built, their comparison to be run again |
| World spawn | verified | Equal on 1,000 seeds to the game's classes, and to ten worlds the official server made |
| Serving a world the official server made (W4) | not started | |

## T3 — Entities

| Feature | Status | Notes |
|---|---|---|
| Entity physics and collision | partial | The game's collider and `Entity.move`: items, falling blocks, vehicles, mobs, projectiles (verified for items, sand and carts). Named: entities tick where a chunk's 3×3 is loaded, not its 5×5; a player's own move is not collided (T0) |
| Mob types | partial | 83 of the game's mobs built with their AI. Not built: the breeze, the creaking, the warden, the wither, the illusioner, the giant, the sulfur cube; their spawn eggs make nothing. Other entities not built: lightning, the fishing bobber, wind charges, the wither skull, display and interaction entities, markers |
| Mob AI and pathfinding | verified | The game's goals, controls, sensing and brains; paths equal node for node to the jar's path finder on 521 layouts; scenes compared with the official server (`mobs_oracle.rs`, `monsters_oracle.rs`, `effects_oracle.rs`, `villagers_oracle.rs`, `nether_mobs_oracle.rs`). Since every player is creative, no mob targets a player |
| Natural spawning and despawning | partial | Every dimension's rules, caps counted across regions, phantoms, patrols, cats, the siege, the wandering trader, zombified piglins from portals. A day of overworld spawning verified (`spawning_oracle.rs`). Monster spawners (`region/spawners.rs`): the delay counted down while a player is within range (a player of another region by their face or the world's table), the tries with the spawner's reason in the rules, the nearby cap, `finalizeSpawn`, the smoke, the poof and the block event, the potentials and custom rules, the state durable in the block entity's data; the clients shown the new mob an egg or the potentials set (`spawners.rs`, its comparison with the official server written and not yet run). Named: draws the game's in distribution, not in value; never thundering; the sulfur cube's spawns in sulfur caves end the whole position; a spawner's entity data beyond its `id` and `Pos` and its equipment are not read, entities that are no built mob not spawned, nearby mobs counted by type, a spawn across a line refused. Not built: trial spawners |
| Combat, damage, effects | partial | A player's hit with strength, crits, sweep and knockback; mobs' attacks, projectiles and effects on mobs and players (verified, `effects_oracle.rs`, `projectiles_oracle.rs`). Players take no damage but the void's (creative); no beacons or conduits |
| Projectiles | verified | Arrows, tridents, crossbows, rockets, potions, snowballs, eggs, pearls (also through gateways), fireballs, llama spit, shulker bullets (`projectiles_oracle.rs`). Named: arrows hit the dragon's whole box, not its parts |
| Vehicles and riding | partial | Minecarts of every kind, boats and rafts, horses, camels, striders, the nautilus, the happy ghast, pigs. Carts and pigs verified (`minecarts_oracle.rs`, `mobs_oracle.rs`); boats and horses not compared. Named: getting on across a line takes four to six ticks |
| TNT and explosions | verified | `tnt_oracle.rs`, the creeper, beds and anchors. Named: explosion drops not merged across a line |
| Items and experience orbs | verified | `entities_oracle.rs`, `items_oracle.rs`, `mobs_oracle.rs`. Named: items do not merge across a line; orbs are not burnt |
| Villagers, trading, raids | verified | N10's villagers, professions, work, beds, gossip, golems, trading, the cure, raids, compared (`merchant_oracle.rs`, `villagers_oracle.rs`, `raids_oracle.rs`, passed 2026-10-28). Named: offers drawn from the villager's own stream; no explorer maps; a raid's waves spawn only in the region's land |
| The ender dragon | verified | The fight, its first moves, its death and what it leaves (`end_oracle.rs`, `dragon_oracle.rs`, `dragon_death_oracle.rs`). Not built: its respawn by four crystals. Named: the pose `DYING` not shown of any dying mob |
| Loot tables and drops | partial | Block and entity loot, bartering, gifts. Not built: structures' chests are not filled when opened; fishing |

## T4 — Redstone

| Feature | Status | Notes |
|---|---|---|
| Dust, torches, repeaters, comparators | verified | With torches' burnout and comparators reading containers (`redstone_oracle.rs`) |
| Pistons, including quasi-connectivity | verified | With slime and honey, pushing entities |
| Observers | verified | |
| Dispensers, droppers, hoppers | partial | The game's dispenser behaviours (a dispenser's shots verified, `projectiles_oracle.rs`) and hoppers moving items. Missing: dispensed spawn eggs, brushes, sheep sheared from a dispenser, fire and wind charges (dropped as items); hoppers and droppers treat furnaces, brewing stands, composters, crafters, chiseled bookshelves, decorated pots, jukeboxes and shelves as shut |
| Rails | implemented | Powered, activator and detector rails |
| Update order and scheduled tick order | verified | The game's order of updates, ticks and block events; a circuit across a pinned line runs as in one region (L6) |
| Plates and tripwire pressed by mobs | implemented | See "Pressure plates and tripwire" in T1 |
| Crafters | not started | |

## T5 — Farms and edge cases

| Feature | Status | Notes |
|---|---|---|
| Mob farms (spawn rules, caps) | partial | The game's rules, light, slime chunks, local and global caps across regions (verified on a flat world, `spawning_oracle.rs`). Monster spawners spawn (`region/spawners.rs`). Farms that use trial spawners, thunder or sulfur cubes do not work |
| Iron, raid and villager-based farms | partial | Golems summoned by gossip and panic, raids, breeding (N10; the bar and countdown verified). Golem summoning not compared |
| Chunk loading, portals | partial | The viewer ring, arrival tickets, the game's portal ticket (300 ticks), the End's arena kept loaded, travel through every portal (verified). Missing: the ender pearl's ticket while it flies, forced loading; pearls kept with an owner who leaves |
| Random ticks and growth | verified | At the game's rate in the chunks a region ticks, and growth by light compared in distribution (`growth_oracle.rs`). Named: draws from the world's own stream; kelp and vine heads stay heads; frosted ice never ages |
| Weather | not started | Always clear: no rain, snow, thunder or lightning, and nothing that depends on them |
