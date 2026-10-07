# Clustine

A from-scratch Minecraft: Java Edition server that simulates **one world across many
instances**. Adding instances adds compute and memory to the same world rather than
creating more separate servers.

> **Status: pre-alpha.** Nothing is playable yet. The first milestone, a single-node
> walking skeleton, is in progress; see the [roadmap](docs/roadmap.md).

## Goals

- **Horizontal scaling of a single world.** The world is split into dynamic regions of
  nearby active chunks; each region is ticked by exactly one worker and can migrate
  between workers while players stay connected.
- **Microservices.** Connection handling, simulation, coordination, storage and world
  generation are separate services, with a single-binary mode for development and small servers.
- **Kubernetes-native.** An operator, autoscaling on tick time, graceful drain and
  zero-downtime rolling updates through live region migration.
- **Its own world format.** Content-addressed sections with a write-ahead log, giving
  deduplication, cheap snapshots and point-in-time restore.
- **Vanilla survival parity** as the long-term target, tracked in a public
  [parity matrix](docs/parity-matrix.md).
- **Plugins** through a cluster-aware native API, with a Bukkit bridge as a later addition.

## Documentation

- [Architecture](docs/architecture.md)
- [Roadmap](docs/roadmap.md)
- [Parity matrix](docs/parity-matrix.md)
- [Library evaluation](docs/library-evaluation.md)
- [Protocol notes for Minecraft 26.3](docs/protocol-26.3.md)
- [Architecture decision records](docs/adr/)

## Repository layout

| Path | Contents |
|---|---|
| `crates/` | Shared libraries: protocol, game data, world model, world format, simulation, region graph, RPC |
| `services/` | One crate per service: edge, coordinator, worker, worldstore, worldgen, playerdata, operator |
| `bin/clustine` | Single-binary mode running every service in one process |
| `tools/` | Development tools: the `botswarm` load-test harness and the `datagen` table generator |
| `docs/` | Architecture, roadmap, parity matrix and decision records |

## Building

Requires Rust 1.85 or newer.

```bash
cargo build --workspace
```

```bash
cargo test --workspace
```

### Running

The server listens on `127.0.0.1:25565` by default and runs in offline mode: names are
not authenticated, so keep it on localhost. So far a client can log in, is put into a
flat creative world and can walk around it. Players do not see each other yet, and
breaking or placing blocks has no effect.

```bash
cargo run -p clustine
```

`botswarm` is the scripted test client:

```bash
cargo run -p clustine-botswarm -- idle 127.0.0.1:25565
```

### Comparing with the official server

The bots and the server share one protocol implementation, so a mistake in it could go
unnoticed between them. The same scenarios can therefore be run against Mojang's server,
and some tests compare both. This needs Java 25, the server jar that `cargo datagen`
downloads, and your agreement to the [Minecraft EULA](https://aka.ms/MinecraftEULA).

```bash
cargo run -p clustine-botswarm -- --vanilla --accept-eula idle
```

```bash
CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test --workspace -- --ignored
```

### Game data

Ids and names of packets, block states, registry entries and tags are committed as
generated Rust tables, so building needs nothing but cargo. To regenerate them, for
example after changing the targeted Minecraft version, run the following. It downloads
the official server jar (about 62 MB) into `target/datagen/` and needs Java 25.

```bash
cargo datagen
```

## Licence

Clustine is free software under the GNU Affero General Public License, version 3 or
later; see [LICENSE](LICENSE) and [ADR-0002](docs/adr/0002-licence.md). If you run a
modified version for players, you have to offer them its source.

---

Not an official Minecraft product. Not approved by or associated with Mojang or Microsoft.
