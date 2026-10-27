# Vanilla parity matrix

Tracks how closely Clustine matches the vanilla server. This is the main place to find
something to work on. Brought up to date on 2026-10-15; its tiers T3 and T4 on 2026-10-27.

Target Minecraft version: **26.3** (protocol 777).

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
how to run it.

- `bin/clustine/tests/oracle.rs`: the same bot scenarios on the official server and
  on Clustine: joining, walking, seeing other players, breaking and placing a block,
  and what the official server does at a second login, with the abilities packet, on
  entering again after leaving in flight, and with light after blocks change.
  Clustine's side of the second login and of entering in flight is held to what
  these showed by `bin/clustine/tests/join.rs`.
- `bin/clustine/tests/placing_oracle.rs`: 205 cases of placing, attaching, using
  doors, trapdoors, fence gates and flower pots, and buckets and lily pads, run on
  both servers and compared block state by block state (step W6 of the terrain plan).
- `services/edge/src/encode.rs`: a flat chunk's packet against the official server's.
- `crates/clustine-light/tests/official.rs`: the light of chunks the official server
  made and lit, against what `clustine-light` gives for their blocks.
- `crates/clustine-terrain/tests/official.rs`: the biomes of chunks the official
  server made.
- The Java judges in `crates/clustine-terrain/tests/java` (`GameBiomes`, `GameFill`,
  `GameSurface`, `GameCarvers`, `GameStructures`) have the pinned jar's own
  generator do one step for a list of chunks, without a server; the tests
  `crates/clustine-terrain/tests/game*.rs` and
  `crates/clustine-structures/tests/game_*.rs` hold the crates to what they wrote,
  block for block.

## T0 — Protocol and session

| Feature | Status | Notes |
|---|---|---|
| Handshake, status, login, configuration | partial | Offline mode only. What a joining client is told (registries, tags, join data) is verified by `join_matches_the_official_server`. With `--seed`, a client is told the hash of the seed, that the world is not flat and that the sea is at 63. A second login as a connected name puts the first connection out with the official server's reason |
| Encryption and compression | partial | Compression is implemented; encryption, which online mode needs, is not |
| Chunk and light data streaming | partial | Flat chunks are verified by `flat_chunk_matches_the_official_server`, the area sent around a player by `walking_matches_the_official_server`. The chunks of a seed (`--seed`) are not compared through the protocol; their blocks and biomes are judged in the crates (T2 below). Chunks are sent in batches paced by the client, but not rate-limited per tick |
| Where a new player enters | partial | The world's spawn is chosen as the official server chooses it for a new world (T8) and kept in the world's `meta`; the client is told it. A new player stands on the spawn column as it is when they enter: on a tower, a fence or the water built or lying there. A player who left enters again where they left, looking the same way and flying if they flew. Missing: the spread of newcomers over the spawn radius, which the official server uses in survival mode, and a spawn moved with `/setworldspawn` |
| Player movement | partial | Positions are taken from the client without checking speed or collisions. The official server's rules of one position per client tick and no movement before "player loaded" are not enforced |
| Seeing other players | partial | Appearing and disappearing are verified by `players_appear_like_on_the_official_server`. Movement is sent as absolute positions every tick; skin layers, equipment, poses and animations are not sent |
| Player list | partial | Names only: no latency, game mode changes or skins |
| Keep-alive and timeouts | implemented | |
| Chat and commands | not started | |

## T1 — Blocks, items, inventory

| Feature | Status | Notes |
|---|---|---|
| Block breaking | partial | Creative mode only: instant, no drops. A door, a bed or a tall plant broken at either half goes whole, and what rested on the block goes with it (phase B3); others nearby see the block break (level event 2001). Verified by `breaking_a_block_matches_the_official_server` and the breaking cases of `placing_oracle.rs` |
| Block placing | partial | Vanilla's common placement rules (W6): a click uses the clicked block before anything is placed (doors, trapdoors and fence gates open and close, a flower goes into a pot, sneaking places instead); a placed block takes the state vanilla gives it (facing, axis, half, wall or floor, two-block doors and beds, double slabs, waterlogging); grass, water and snow are replaced; plants go only where they can grow. Verified by the 205 cases of `placing_oracle.rs`. Not yet: signs do not open their editor; chests, crafting tables, furnaces, buttons and levers do nothing when clicked; the classes whose survival rule is not ported place nothing; crops and mushrooms are placed whatever the light; the rest of the known limits of the roadmap |
| Block state updates and neighbour updates | partial | Phase B ([ADR-0024](adr/0024-block-behaviour-in-a-region.md)): a change updates its neighbours as `Level.setBlock` does, depth first (`updateShape`, `neighborChanged`): fences, panes, walls and stairs join back, what rests on or hangs from a block goes with it (torches, ladders, signs, plants, doors, rails, snow), others nearby see it break. Scheduled ticks run, durable across merges, splits and crashes; sand and gravel fall, landing at once. Generated land is post-processed as it first ticks and shown settled. An update that would cross into another region's land waits as a seam entry until one region has both sides. A chest placed beside a chest facing the same way pairs with it (single when sneaking), and the other half of a double chest broken is single again; copper and trapped chests pair only with their own kind. A rail joins the rails beside it, curving and sloping up a step, and turns them towards itself; powered, detector and activator rails stay straight; a sloped rail goes with the block it rises onto (B7). Verified by `placing_oracle.rs`'s breaking cases against the official server, and its cases of chests and rails written to be. Not yet: redstone power (T4), so a rail at a junction never takes the curve power would give it and powered rails are never powered |
| Inventory and containers | partial | The nine hotbar slots, filled from the creative inventory and kept across leaving; item components are dropped. A bucket filled in creative goes into the first empty hotbar slot, where vanilla puts it anywhere in the inventory |
| Crafting, smelting, other recipes | not started | |
| Item components and enchantments | not started | |
| Fluids | partial | Buckets place and take water and lava sources, and lily pads go on water. Water and lava flow by `FlowingFluid`'s rules (B5): spread, drop-off, the slope search, sources between sources, waterlogging, lava meeting water making obsidian, cobblestone, stone and basalt with its hiss, concrete powder hardening. Verified by `placing_oracle.rs`'s flowing cases against the official server, settled. Not yet: bubble columns; what water washes away drops nothing (no items) |
| Lighting | verified | Sky and block light from `clustine-light`, made with the chunk, equal to the official server's for chunks it made (`crates/clustine-light/tests/official.rs`). Where blocks have changed the edge lights the chunk anew and sends light updates as the official server does ([ADR-0023](adr/0023-light-at-the-edge.md); `light_after_blocks_change_on_the_official_server`) |

## T2 — World generation

Everything here is in the crates `clustine-terrain`, `clustine-features` and
`clustine-structures`, and is served for the overworld by `--seed`; a server without a
seed is flat. Phases and steps are those of the
[terrain plan](groundwork/terrain-plan.md).

| Feature | Status | Notes |
|---|---|---|
| Biomes | verified | T3. Equal to the chunks the official server made on seed 13579 but for four exact ties, and to the game's own classes in all three dimensions (`GameBiomes`) |
| Overworld terrain (bit-exact for a seed) | partial | The fill from noise with its aquifers (T4), the surface and the ore veins (T5), the carvers (T6) and the ground bent under structures (T7) are built and equal to the game's own classes block for block (`GameFill`, `GameSurface`, `GameCarvers`, `GameStructures`). What the features stage places is the row below |
| Carvers | verified | T6: caves and ravines, judged by the game's own classes |
| Features | partial | The overworld's, placed in ADR-0021's order of runs and equal to the official server's chunk for chunk over an area of each of the 56 overworld biomes on two seeds (`game_features.rs`, phase F of the terrain plan): trees, plants, ores, lakes, springs, geodes, dripstone, sculk, fossils and the rest, with their block entities, ticks and marks. Named exceptions: a surface block whose biome cell is an exact tie may follow the other biome, as the game's surface answers ties afresh; icebergs use `Math.sin` and `Math.cos`, which HotSpot on x86-64 computes with intrinsics of its own that differ from fdlibm (`StrictMath`, Clustine's `libm`) in the last bit for about one angle in fifteen: where a block of the berg then falls on the other side of its edge, the draws after it shift and the cut through the berg moves (in 204 areas of two seeds with structures on, 2026-10-16: one block in one area and 571 in another, both on seed 12345). A server on ARM is believed to compute them as `StrictMath` does, as Clustine does. Fossils, desert wells and sulfur springs need the game's jar for their templates. Not built: the Nether's and the End's features (with phase D); entities features would make are left out |
| Structures | partial | T7: which structures start where, with every piece and its place, and the ground bent under them, judged by the game's own classes. Their buildings (S0 to S2): every overworld structure's blocks, block entities, ticks and marks, judged with the features over the structures' areas of two seeds (`game_features.rs`). Entities they would hold (villagers, the mansion's illagers) are left out with the rest of T3. Not built: S3 |
| Nether | partial | Biomes, fill, surface and carvers are in the crate and judged by the game's own classes; no features, and not served (phase D) |
| End | partial | As the Nether |

## T3 — Entities

Phase N of the roadmap ([ADR-0028](adr/0028-entities.md)); each part's named differences
are in its plan.

| Feature | Status | Notes |
|---|---|---|
| Entity physics and collision | partial | The game's collider and `Entity.move` (N4): items, falling blocks, vehicles, mobs; entities kept in their chunks, crossing lines between regions whole (N3). Entities tick where a chunk's 3x3 is loaded or copied, not its 5x5 |
| Natural spawning and despawning | partial | Every overworld type's rule under the caps counted across regions (N9); phantoms, patrols, cats, the siege and the wandering trader (N9, N10). Draws are the game's in distribution, not in value. Not yet: the Nether's and the End's |
| Mob AI and pathfinding | verified | The game's goals, controls, sensing and brains; paths judged node for node against the jar's own path finder on 521 layouts (`crates/clustine-sim/tests/paths_official_server.rs`); a cow made, hit and bred compared with the official server (`mobs_oracle.rs`) |
| Combat, damage, status effects | partial | The living body's hurts, death, loot and experience (N7); every monster's attacks, projectiles, mob effects on mobs and players, eating and drinking (N9). Players are in creative mode: survival's damage to players waits for survival |
| Villagers and trading | partial | N10: the villager's brain, job sites, professions, work, beds, gossip, golems, raids, trading from the game's tables, the merchant window, the cure. The comparisons with the official server are written (`merchant_oracle.rs`, `villagers_oracle.rs`, `raids_oracle.rs`) and not yet run |
| Loot tables and drops | partial | Block and entity loot tables; structures' chests are empty |

## T4 — Redstone

Phase R of the roadmap ([ADR-0026](adr/0026-redstone.md)) and lending at a line
([ADR-0027](adr/0027-lending-at-a-line.md)).

| Feature | Status | Notes |
|---|---|---|
| Dust, torches, repeaters, comparators | verified | With torches' burnout, comparators reading containers; eleven circuits send the same packets on the same ticks as the official server (`redstone_oracle.rs`) |
| Pistons, including quasi-connectivity | verified | Pistons and sticky pistons with slime and honey, pushing entities; in the same circuits |
| Observers, droppers, dispensers, hoppers | implemented | With the game's dispenser behaviours and hoppers moving items (N6) |
| Update order and scheduled tick order | verified | The game's order of updates, ticks and block events, from its classes, in the same circuits; a circuit across a pinned line runs as in one region (L6) |

## T5 — Farms and edge cases

| Feature | Status | Notes |
|---|---|---|
| Mob farms (spawn rules, caps) | not started | |
| Iron, raid and villager-based farms | not started | |
| Chunk loading behaviour, portals | not started | |
| Random tick and crop growth rates | partial | Random ticks at the game's rate (three positions a section a tick) in the chunks a region ticks (B7): leaves decay, sugar cane, cactus, kelp, vines, cocoa and nether wart grow, copper weathers, amethyst and dripstone grow, farmland dries, fire burns and spreads, lava sets fire, coral dies. Drawn from a stream of the world's own, not the game's. Not yet: what needs light (crops, saplings, grass, mushrooms, bamboo, ice and snow), which waits for light in the simulation; rain |
