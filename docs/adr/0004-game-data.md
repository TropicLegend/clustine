# ADR-0004: Generated game data is committed

- Status: **Accepted**; amended on 2026-10-10 by
  [ADR-0019](0019-data-made-from-mojangs-jar.md), which lets more than ids and names
  be committed: tables of what the jar's code says of blocks and biomes (the light a
  block gives, its shapes, its fluid, its class, which climate is which biome), the
  Rust that `cargo datagen` emits from the jar's world-generation data (noise routers,
  material rules, carvers, structures and template pools), and of each structure
  template its size and jigsaw blocks, all with a notice that the data is Mojang's and
  not under the AGPL. The jar, its reports, its data pack and the templates' blocks
  are still never committed.
- Date: 2026-10-07

## Context

Packet ids, block state ids, registry entry names and tags have to match the targeted
Minecraft version exactly. The authoritative source is the data generator built into the
official server jar. The jar and its data pack are Mojang's and are not redistributed.

Generating at build time would put a network download and a Java runtime into every
build, including CI and every contributor's first `cargo build`.

## Decision

- `tools/datagen` downloads the server jar for the targeted version into an ignored cache
  under `target/`, verifies its checksum, runs the data generator, and writes Rust source
  into `crates/clustine-data/src/generated/` and `crates/clustine-protocol/src/generated/`.
- The generated Rust tables contain ids and names only and **are committed**.
- The jar, the generator's reports and the data pack JSON are never committed.
- `datagen --check` regenerates and fails if the committed tables differ.

## Consequences

- Building and testing needs only cargo; builds work offline.
- Moving to a new Minecraft version is a reviewable diff of the generated tables.
- Only someone updating the tables needs Java and the download.
- Hand-written code must not duplicate values that the tables provide.
