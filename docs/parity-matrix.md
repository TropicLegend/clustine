# Vanilla parity matrix

Tracks how closely Clustine matches the vanilla server. This is the main place to find
something to work on. Brought up to date on 2026-10-15.

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
| Block breaking | partial | Creative mode only: instant, no drops. Breaking one half of a door, bed or tall plant leaves the other (phase B2 of the terrain plan). Verified for a simple case by `breaking_a_block_matches_the_official_server` |
| Block placing | partial | Vanilla's common placement rules (W6): a click uses the clicked block before anything is placed (doors, trapdoors and fence gates open and close, a flower goes into a pot, sneaking places instead); a placed block takes the state vanilla gives it (facing, axis, half, wall or floor, two-block doors and beds, double slabs, waterlogging); grass, water and snow are replaced; plants go only where they can grow. Verified by the 205 cases of `placing_oracle.rs`. Not yet: signs do not open their editor; chests, crafting tables, furnaces, buttons and levers do nothing when clicked; the classes whose survival rule is not ported place nothing; crops and mushrooms are placed whatever the light; the rest of the known limits of the roadmap |
| Block state updates and neighbour updates | partial | A placed block takes its shape from its neighbours as it is placed (fences, panes, walls, stairs, redstone wire). The neighbours do not change in turn: a fence beside a new fence does not join back, chests stay single, rails do not curve (phase B1), and nothing falls or breaks for want of support (B2, B3) |
| Inventory and containers | partial | The nine hotbar slots, filled from the creative inventory and kept across leaving; item components are dropped. A bucket filled in creative goes into the first empty hotbar slot, where vanilla puts it anywhere in the inventory |
| Crafting, smelting, other recipes | not started | |
| Item components and enchantments | not started | |
| Fluids | partial | Buckets place and take water and lava sources, and lily pads go on water (verified in `placing_oracle.rs`). Nothing flows (phase B5) |
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
| Features | partial | The overworld's, placed in ADR-0021's order of runs and equal to the official server's chunk for chunk over an area of each of the 56 overworld biomes on two seeds (`game_features.rs`, phase F of the terrain plan): trees, plants, ores, lakes, springs, geodes, dripstone, sculk, fossils and the rest, with their block entities, ticks and marks. Named exceptions: a surface block whose biome cell is an exact tie may follow the other biome, as the game's surface answers ties afresh; icebergs use `Math.sin` and `Math.cos`, which HotSpot computes otherwise than fdlibm for about one angle in fifteen (one block of one iceberg in 110 areas). Fossils, desert wells and sulfur springs need the game's jar for their templates. Not built: the Nether's and the End's features (with phase D); entities features would make are left out |
| Structures | partial | T7: which structures start where, with every piece and its place, and the ground bent under them, judged by the game's own classes. No block of a structure is placed (phase S) |
| Nether | partial | Biomes, fill, surface and carvers are in the crate and judged by the game's own classes; no features, and not served (phase D) |
| End | partial | As the Nether |

## T3 — Entities

| Feature | Status | Notes |
|---|---|---|
| Entity physics and collision | not started | |
| Natural spawning and despawning | not started | |
| Mob AI and pathfinding | not started | |
| Combat, damage, status effects | not started | |
| Villagers and trading | not started | |
| Loot tables and drops | not started | |

## T4 — Redstone

| Feature | Status | Notes |
|---|---|---|
| Dust, torches, repeaters, comparators | not started | Redstone wire is placed with its shape from its neighbours; nothing is powered |
| Pistons, including quasi-connectivity | not started | |
| Observers, droppers, dispensers, hoppers | not started | |
| Update order and scheduled tick order | not started | |

## T5 — Farms and edge cases

| Feature | Status | Notes |
|---|---|---|
| Mob farms (spawn rules, caps) | not started | |
| Iron, raid and villager-based farms | not started | |
| Chunk loading behaviour, portals | not started | |
| Random tick and crop growth rates | not started | |
