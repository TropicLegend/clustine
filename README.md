# Clustine

A from-scratch Minecraft: Java Edition server that simulates **one world across many
instances**. Adding instances adds compute and memory to the same world rather than
creating more separate servers.

> **Status: pre-alpha.** Nothing is playable. The repository currently contains the
> workspace skeleton and design documents only. See the [roadmap](docs/roadmap.md).

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
- [Architecture decision records](docs/adr/)

## Repository layout

| Path | Contents |
|---|---|
| `crates/` | Shared libraries: protocol, game data, world model, world format, simulation, region graph, RPC |
| `services/` | One crate per service: edge, coordinator, worker, worldstore, worldgen, playerdata, operator |
| `bin/clustine` | Single-binary mode running every service in one process |
| `tools/` | Development tools, starting with the `botswarm` load-test harness |
| `docs/` | Architecture, roadmap, parity matrix and decision records |

## Building

Requires Rust 1.85 or newer.

```bash
cargo build --workspace
```

```bash
cargo test --workspace
```

## Licence

Not yet decided; see [ADR-0002](docs/adr/0002-licence.md). Until a licence is added, no
rights are granted to use, modify or redistribute this code.

---

Not an official Minecraft product. Not approved by or associated with Mojang or Microsoft.
