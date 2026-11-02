# Clustine

A from-scratch Minecraft: Java Edition server that simulates **one world across many
instances**. Adding instances adds compute and memory to the same world rather than
creating more separate servers.

> **Status: pre-alpha.** Players play a creative world with its overworld, Nether and
> End, flat or with `--seed` the game's own land for that seed, block for block. Blocks
> are placed, used and updated by vanilla's rules: water and lava flow, sand falls,
> plants grow by their light, redstone runs with the game's order of updates. Animals,
> monsters and villagers live with the game's AI and spawn by its rules; villagers trade
> and raids come; minecarts, boats and mounts ride; containers open with the game's
> menus. The Nether's portals and the End's dragon fight, gateways and credits are
> built. Most of this is compared with Mojang's own server ([parity
> matrix](docs/parity-matrix.md)). Not yet: survival mode (health, hunger, breaking
> times and drops, crafting, smelting), chat and commands, weather, and online mode.
> The world is simulated in regions that follow the players, split when a group goes
> off on its own and merged when it comes back, and shared out among several worker
> processes, with no seam where regions meet; a worker or the world store can die
> without anyone being disconnected or losing anything they were shown. A player who
> leaves comes back where they left. There is one coordinator. See the
> [roadmap](docs/roadmap.md), whose first section says where the work stands.

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
- [Architecture decision records](docs/adr/README.md)
- [Plans and studies before a milestone](docs/groundwork/), among them the
  [terrain plan](docs/groundwork/terrain-plan.md)

## Repository layout

| Path | Contents |
|---|---|
| `crates/` | Shared libraries: protocol, game data, world model, world format, simulation, regions and who runs them, RPC; light (`clustine-light`); the rules of blocks the simulation and generation share (`clustine-blocks`); world generation (`clustine-noise`, `clustine-terrain`, `clustine-features` for the game's features and structures' pieces, `clustine-structures`, and `clustine-worldgen-data`, emitted from the game's data) |
| `services/` | One crate per service: edge, coordinator, worker, worldstore, worldgen (the generator and its scheduler); `playerdata` and `operator` are empty so far |
| `bin/clustine` | Single-binary mode running every service in one process |
| `tools/` | Development tools: the `botswarm` load-test harness, the `datagen` table generator, `check.sh`, which runs every check CI runs, and `oracles.sh`, which runs the comparisons with the official server |
| `deploy/` | Kubernetes manifests and the test that runs a cluster in kind |
| `docs/` | Architecture, roadmap, parity matrix and decision records |

## Building

Requires Rust 1.89 or newer (a dependency, `uuid`, needs it; the workspace itself says
1.88).

```bash
cargo build --workspace
```

```bash
cargo test --workspace
```

### Running

The server listens on `127.0.0.1:25565` by default and runs in offline mode: names are
not authenticated, so keep it on localhost. Every player is in creative mode. A player's
place is kept when they leave: where they were, in which dimension, flying or not, their
whole inventory and ender chest, experience, effects, health and their respawn point.
The world is kept in the directory `world` (see `--world`). Changes are logged as they
happen, so they survive the server being killed. Every world has its Nether and its
End; a flat world's are those of seed 0, as the game's flat world has them.

```bash
cargo run -p clustine
```

The world begins with a home region for each dimension. A group of players that goes far from everybody else
(32 chunks at the usual view distance) is split off into a region of its own, which is
simulated on a thread of its own, and merged again when it comes back within 24
chunks. Each is a pause for those it concerns, and the log says when it happens: in an
optimised build (`--release`) well under a second, also for a crowd of a hundred
(measured with the world on network storage: the
[roadmap](docs/roadmap.md) has the numbers).

Regions can also be pinned side by side, by naming the chunk x coordinates where they
meet, to have a boundary at a known place. Players are handed from region to region as
they walk, and build across the boundary, without noticing it. This one is at block
x = 64:

```bash
cargo run -p clustine -- --pin 4 --reshape by-hand
```

With a seed the world is not flat but the game's own for that seed, in its three
dimensions: the overworld's mountains, coasts, oceans, rivers, caves and ravines with
its biomes, everything the game's features place and its structures' buildings; the
Nether's land with its fortresses and bastions; the End's island, outer islands and
cities. Players enter where the game would put a new world's spawn for that seed, which
the world keeps from when it is made. Structures built of the game's own templates
(villages, bastions, ruined portals, shipwrecks, igloos, ocean ruins, mansions, End
cities, fossils, desert wells and others) are Mojang's and not in Clustine: they are
read from the game's server jar, the one `cargo datagen` keeps under `target/` or
another named with `--minecraft-jar`, and without one they are left out (a flat
world reads it too, for its Nether and its End). Chunks are made on a pool of threads as
they are first asked for and kept, so a second visit reads them from the disk. A world
is made one way for good, so give it a directory of its own; one that holds a flat
world, a world of another seed, or one made by an earlier build of Clustine's land, is
refused.

```bash
cargo run --release -p clustine -- --seed 13579 --world world-13579
```

### Running a cluster

The same binary is each service of a cluster when given a subcommand. This starts a
world on two workers on one machine; the processes find each other on their default
ports and can be started in any order. A group that is split off gets a region that
is moved to the other worker ten seconds later:

```bash
cargo run -p clustine -- coordinator
```

```bash
cargo run -p clustine -- worldstore --world world
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

Players connect to the edge on `127.0.0.1:25565` once a worker runs the region players
enter the world in, which takes a moment: a coordinator gives nothing away for the
first five seconds (its lease, `--lease-seconds`). With `coordinator --reshape by-hand` and `worldstore --pin 4` the
overworld is two pinned regions, one for each worker (the Nether and the End have a
region each), and nothing merges or splits unless
it is asked for (`clustine merge`, `clustine split`, `clustine move`). For a world with
a seed, give the same `--seed` to the world store (which makes the chunks and puts
the spawn where the game would, keeping it with the world) and every edge (which tells
clients of the world); nothing checks that they agree. The coordinator learns where
players enter from the store. The services do
not authenticate each other, so their ports are for a private network only.

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
CLUSTINE_ACCEPT_MINECRAFT_EULA=true tools/oracles.sh
```

`tools/oracles.sh` runs the comparisons one test target at a time, each under a limit of
its own, and says which failed; the [parity matrix](docs/parity-matrix.md) lists them.

### Game data

Ids and names of packets, block states, registry entries and tags, and what the game's
code says of every block state (its light, its shapes, its fluid), are committed as
generated Rust tables and packed tables, so building needs nothing but cargo. To
regenerate them, for example after changing the targeted Minecraft version, run the
following. It downloads the official server jar (about 62 MB) into `target/datagen/`,
runs the jar's data generator, and compiles and runs a Java program of Clustine's own
against the jar's classes without starting a server. It needs a JDK 25 (`javac` and
`java` on the path), where a Java runtime was enough before.

```bash
cargo datagen
```

`cargo datagen --check` fails if the committed output is not, byte for byte, what the
pinned jar gives; `tools/check.sh` runs it where the jar is already in `target/datagen/`.
`tools/datagen/generated.sums` holds the hashes of what the output was made from, and
an ordinary test fails when datagen's sources or `Cargo.lock` changed and it was not
run again, so run it after such a change. `cargo datagen --dump block_states` prints a
packed table as text. What is committed and why is in
[ADR-0019](docs/adr/0019-data-made-from-mojangs-jar.md).

## Licence

Clustine is free software under the GNU Affero General Public License, version 3 or
later; see [LICENSE](LICENSE) and [ADR-0002](docs/adr/0002-licence.md). If you run a
modified version for players, you have to offer them its source.

Game data that is generated from Mojang's server and committed is Mojang's; it is not
under this licence and Clustine grants no rights to it. See [NOTICE.md](NOTICE.md).

---

Not an official Minecraft product. Not approved by or associated with Mojang or Microsoft.
