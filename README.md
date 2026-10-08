# Clustine

A from-scratch Minecraft: Java Edition server that simulates **one world across many
instances**. Adding instances adds compute and memory to the same world rather than
creating more separate servers.

> **Status: pre-alpha.** Players walk around a flat creative world, see each other and
> build; that world can be simulated by several worker processes, with players handed
> from one to the next as they walk; and a worker or the world store can die without
> anyone being disconnected or losing anything they were shown: another worker carries
> on with the region from what is on disk. The regions are still fixed parts of the
> world, shared out among the workers, and there is one edge and one coordinator. See the
> [roadmap](docs/roadmap.md).

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
- [World format](docs/world-format.md)
- [Protocol notes for Minecraft 26.3](docs/protocol-26.3.md)
- [Architecture decision records](docs/adr/)

## Repository layout

| Path | Contents |
|---|---|
| `crates/` | Shared libraries: protocol, game data, world model, world format, simulation, region graph, RPC |
| `services/` | One crate per service: edge, coordinator, worker, worldstore, worldgen, playerdata, operator |
| `bin/clustine` | Single-binary mode running every service in one process |
| `tools/` | Development tools: the `botswarm` load-test harness and the `datagen` table generator |
| `deploy/` | Kubernetes manifests and the test that runs a cluster in kind |
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
not authenticated, so keep it on localhost. So far players can log in, are put into a
flat creative world, can walk around it, see each other, and break and place blocks.
The world is kept in the directory `world` (see `--world`). Changes are logged as they
happen, so they survive the server being killed.

```bash
cargo run -p clustine
```

The world can be divided into regions that are simulated separately, each on a thread
of its own, by naming the chunk x coordinates where they meet. Players are handed from
region to region as they walk, and build across the boundaries, without noticing them:

```bash
cargo run -p clustine -- --boundaries 4
```

### Running a cluster

The same binary is each service of a cluster when given a subcommand. This starts a
world of two regions on one machine; the processes find each other on their default
ports and can be started in any order:

```bash
cargo run -p clustine -- coordinator --boundaries 4
```

```bash
cargo run -p clustine -- worldstore --world world --boundaries 4
```

```bash
cargo run -p clustine -- worker --name worker-0 --listen 127.0.0.1:25601
```

```bash
cargo run -p clustine -- worker --name worker-1 --listen 127.0.0.1:25611
```

```bash
cargo run -p clustine -- edge
```

Players connect to the edge on `127.0.0.1:25565` once both workers have a region, which
takes a moment: a coordinator gives nothing away for the first ten seconds. The services
do not authenticate each other, so their ports are for a private network only.

[deploy/](deploy/README.md) has the same as Kubernetes manifests, and a test that runs
it in a local cluster.

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
