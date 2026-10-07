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

## T0 — Protocol and session

| Feature | Status | Notes |
|---|---|---|
| Handshake, status, login, configuration | not started | |
| Encryption and compression | not started | |
| Chunk and light data streaming | not started | |
| Player movement | not started | |
| Chat and commands | not started | |

## T1 — Blocks, items, inventory

| Feature | Status | Notes |
|---|---|---|
| Block placing and breaking | not started | |
| Block state updates and neighbour updates | not started | |
| Inventory and containers | not started | |
| Crafting, smelting, other recipes | not started | |
| Item components and enchantments | not started | |
| Fluids | not started | |
| Lighting | not started | |

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
