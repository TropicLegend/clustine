# Vanilla parity matrix

Tracks how closely Clustine matches the vanilla server. This is the main place to find
something to work on.

Target Minecraft version: **26.3** (protocol 777).

## Status values

| Status | Meaning |
|---|---|
| not started | No implementation |
| partial | Implemented, known differences from vanilla (link the issues) |
| implemented | Believed complete, not yet covered by a differential test |
| verified | Covered by a differential test against the vanilla server that passes |

The differential tests are the ignored tests in `bin/clustine/tests/oracle.rs` and
`services/edge/src/encode.rs`; the README says how to run them.

## T0 — Protocol and session

| Feature | Status | Notes |
|---|---|---|
| Handshake, status, login, configuration | partial | Offline mode only. What a joining client is told (registries, tags, join data) is verified by `join_matches_the_official_server` |
| Encryption and compression | partial | Compression is implemented; encryption, which online mode needs, is not |
| Chunk and light data streaming | partial | Flat chunks are verified by `flat_chunk_matches_the_official_server`, the area sent around a player by `walking_matches_the_official_server`. Chunks are sent in batches paced by the client, but not rate-limited per tick |
| Player movement | partial | Positions are taken from the client without checking speed or collisions. The official server's rules of one position per client tick and no movement before "player loaded" are not enforced |
| Seeing other players | partial | Appearing and disappearing are verified by `players_appear_like_on_the_official_server`. Movement is sent as absolute positions every tick; skin layers, equipment, poses and animations are not sent |
| Player list | partial | Names only: no latency, game mode changes or skins |
| Keep-alive and timeouts | implemented | |
| Chat and commands | not started | |

## T1 — Blocks, items, inventory

| Feature | Status | Notes |
|---|---|---|
| Block breaking | partial | Creative mode only: instant, no drops. Verified for a simple case by `breaking_a_block_matches_the_official_server` |
| Block placing | partial | The block's default state is placed against the clicked face into air: no orientation, no replacing of plants or fluids, no interaction with the clicked block. Verified for a simple case by `placing_a_block_matches_the_official_server` |
| Block state updates and neighbour updates | not started | A placed or broken block does not affect its neighbours |
| Inventory and containers | partial | The nine hotbar slots, filled from the creative inventory; item components are dropped |
| Crafting, smelting, other recipes | not started | |
| Item components and enchantments | not started | |
| Fluids | not started | |
| Lighting | partial | Sky light only, per column: exact for terrain without overhangs, too dark below them. The client relights blocks that change |

## T2 — World generation

| Feature | Status | Notes |
|---|---|---|
| Overworld terrain (bit-exact for a seed) | not started | |
| Biomes | not started | |
| Carvers and features | not started | |
| Structures | not started | |
| Nether | not started | |
| End | not started | |

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
| Dust, torches, repeaters, comparators | not started | |
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
