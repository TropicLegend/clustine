# Library evaluation

Which existing Rust libraries Clustine can reuse for concerns outside the simulation core.

Researched on 2026-10-07 from the crates.io API, the GitHub API and repository files.
Versions and licences change; re-check before adding a dependency. Licence statements
are a reading of the published metadata, not legal advice.

Reference point: the latest Minecraft: Java Edition release is **26.3** (2026-09-15,
protocol 777).

## Summary

| Concern | Recommendation | Reason |
|---|---|---|
| Protocol codec | Write our own | Nothing is at once permissively licensed, current, on stable Rust and free of a parent project's runtime |
| NBT | `fastnbt` | Permissive, actively released, stable Rust |
| Game data | Generate from the server jar | Same approach as Azalea and Hyperion; existing data crates are stale, nightly-only or GPL |
| World generation | Reuse `steel-worldgen` or `pumpkin-world`; choose when world generation starts | The only two modern vanilla implementations in Rust are AGPL-3.0 and GPL-3.0 |
| Load testing | Azalea swarms, kept outside the stable workspace | MIT, but requires nightly Rust and lags new Minecraft versions |
| Infrastructure | No obstacles | All candidates are permissively licensed and maintained |

The world generation finding weighed heavily in [ADR-0002](adr/0002-licence.md), which
chose AGPL-3.0-or-later after this evaluation was written. Statements below about what a
permissive licence would rule out describe the options at that time; with the AGPL, the
copyleft crates listed here are usable.

## Parent project licences

| Project | Licence | Notes |
|---|---|---|
| Pumpkin | GPL-3.0 | MIT until 2026-02-01. Its plugin API crates are stated as MIT OR Apache-2.0 |
| SteelMC | AGPL-3.0-or-later | |
| Valence | MIT | |
| Azalea | MIT | |
| FerrumC | MIT | Crate manifests carry no licence field |
| Hyperion | Apache-2.0 | Changed several times in 2024; a vendored bot tool inside it is GPL-3.0-or-later |

## Protocol codec

| Crate | Licence | Minecraft | Published | Notes |
|---|---|---|---|---|
| `pumpkin-protocol` | GPL-3.0 | 26.3 | No (git only) | Stable Rust; the only candidate on 26.3 |
| `azalea-protocol` | MIT | 26.1 released, 26.2 on main | Yes | Nightly Rust only; pulls in about ten sibling crates; supports one version at a time |
| `hyperion-minecraft-proto` | Apache-2.0 | 26.2 | No | Fewest dependencies; generated from the data generator and a decompiled jar |
| `steel-protocol` | AGPL-3.0-or-later | 26.2 | No | Pinned nightly |
| `valence_protocol` | MIT | 1.20.1 | Pre-release from 2023 | Mandatory `bevy_ecs` dependency |
| FerrumC net crates | MIT (repository) | 1.21.8 | No | Tightly coupled; project is being rewritten |

Decision: `clustine-protocol` is written in-house. Packet ids and names come from the data
generator's `packets.json`; field layouts are not in that report and have to be written by
hand, using `hyperion-minecraft-proto` and `azalea-protocol` as references.

## NBT

| Crate | Licence | Latest | Notes |
|---|---|---|---|
| `fastnbt` | MIT OR Apache-2.0 | 2.6.3 (2026-08-08) | serde-based, stable Rust |
| `simdnbt` | MIT | 0.10.0 (2026-03-28) | Fastest; nightly Rust and 64-bit only |
| `valence_nbt` | MIT | 0.8.0 (2023-10-09) | No release in three years |
| `quartz_nbt` | MIT | 0.2.9 (2024-03-19) | Dormant |
| `pumpkin-nbt` | GPL-3.0 | Not published | |

## Game data

| Source | Licence | Minecraft | Notes |
|---|---|---|---|
| Server jar data generator | Mojang's data, generated locally | Any | See below |
| Pumpkin `Extractor` (Fabric mod) | MIT | Current | Extracts what the reports lack |
| `pumpkin-data` | GPL-3.0 | 26.3 | Not published; build script downloads the server jar |
| `azalea-registry`, `azalea-block` | MIT | 26.1 / 26.2 | Nightly Rust through their dependencies |
| `valence_generated` | MIT | 1.20.1 | Stale |
| PrismarineJS `minecraft-data` | MIT per README, no licence file | Full data up to 26.1 | Plain JSON; two releases behind |

The data generator is run with:

```bash
java -DbundlerMainClass=net.minecraft.data.Main -jar minecraft_server.jar --all --output generated
```

It writes `reports/blocks.json` (every block state with its numeric id),
`reports/registries.json`, `reports/commands.json`, `reports/packets.json`, and the
vanilla data pack under `data/` (tags, recipes, loot tables, world generation JSON).
This has not yet been run for 26.3 as part of this evaluation; the exact file list and the
required Java version are to be confirmed when `clustine-data` is started.

## World generation

| Crate | Licence | Minecraft | Standalone | Stated completeness |
|---|---|---|---|---|
| `steel-worldgen` (SteelMC) | AGPL-3.0-or-later | 26.2 | Fairly isolated | Project states 7,500 random chunks across three dimensions match vanilla block for block; pinned nightly |
| `pumpkin-world` | GPL-3.0 | 26.3 | No; also contains chunk I/O, lighting and ticking | Feature checklist complete; no block-for-block claim found |
| `ferrumc-world-gen` | MIT (repository) | 1.21.8 | No | Not vanilla terrain |
| `cubiomes` bindings | MIT | Not checked | Yes (C library) | Biomes and seed finding only |

No permissively licensed Rust implementation of modern vanilla terrain was found.
Neither project's parity claim has been reproduced here.

Clustine is AGPL-3.0-or-later, so either crate can be reused, which removes most of parity
tier T2. Which one is decided when world generation starts.

## Load testing

| Tool | Licence | Notes |
|---|---|---|
| `azalea` | MIT | Rust bot library with swarm support; nightly Rust; one maintainer; released for 26.1 |
| FerrumC `tools/stress-bot` | MIT (repository) | Working Azalea swarm tool, usable as a template |
| SoulFire (Java) | AGPL-3.0 | Most active dedicated stress tool; usable as a separate process |
| `rust-mc-bot` | GPL-3.0 | Minimal stress client; usable as a separate process |
| mineflayer (JavaScript) | MIT | Supports up to 26.1 |

Because Azalea needs nightly Rust, `botswarm` must not force nightly on the rest of the
workspace. Options to settle in M1: keep `botswarm` in its own workspace with its own
toolchain, or build its bots on `clustine-protocol` and use Azalea or a real client only
as an independent check.

## Infrastructure

| Crate | Licence | Latest stable | Notes |
|---|---|---|---|
| `openraft` | MIT OR Apache-2.0 | 0.9.25 | 0.10 is still alpha |
| `kube` | Apache-2.0 | 4.2.0 | Minimum Rust 1.89 |
| `tonic` | MIT | 0.14.6 | |
| `quinn` | MIT OR Apache-2.0 | 0.11.12 | |
| `wasmtime` | Apache-2.0 WITH LLVM-exception | 49.0.2 | Minimum Rust 1.96 |
| `zstd` | BSD-3-Clause | 0.14.0 | Was MIT up to 0.13 |
| `bevy_ecs` | MIT OR Apache-2.0 | 0.19.1 | |
| `hecs` | MIT OR Apache-2.0 | 0.11.2 | |
| `flecs_ecs` | MIT | 0.2.2 | Releases lag the repository |

Several of these need a newer compiler than the workspace's current minimum of 1.85; the
minimum is raised when the first of them is added.

## Not verified

- Whether `azalea-protocol` can be used on the server side.
- The world generation parity claims of SteelMC and Pumpkin.
- Whether `hyperion-minecraft-proto` builds on stable Rust.
- Licences of the C libraries wrapped by `zstd` and `flecs_ecs`.
- "Maintained" for the infrastructure crates is judged from release dates only.
