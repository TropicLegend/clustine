# Current state

Updated 2026-11-13 from the repository state.

## What exists

Clustine is a Rust Minecraft: Java Edition 26.3 server that runs one world across worker
processes. It has a single-binary mode, separate services, durable regions, player
handover, chunk generation, and the overworld, Nether, and End. Most implemented gameplay
is compared with the official server or the game's own classes; see the
[parity matrix](parity-matrix.md).

Players currently join in creative mode by default. Survival is in progress: phases V1 to
V4 cover per-player game modes, survival digging and crafting, health, hunger, damage,
death, and respawn. **V5 (furnaces) is next**; the plan makes survival the default after
V5. The remaining V6 to V13 work is listed in the
[survival plan](groundwork/v-plan.md). Other player-visible gaps are summarized at the
top of the [parity matrix](parity-matrix.md).

Separate planned work for the Kubernetes operator and rolling updates, dense-crowd
scaling, and plugins has not begun; their milestones are in the [roadmap](roadmap.md).

## Where to start in the code

| Task | Start here |
|---|---|
| Minecraft packets, client sessions, routing, fan-out | `services/edge/` |
| Region ticking and orchestration | `services/worker/` |
| Game rules and deterministic region simulation | `crates/clustine-sim/` |
| World persistence and region ownership in storage | `services/worldstore/` |
| Region ownership decisions | `services/coordinator/` |
| Packet and RPC contracts | `crates/clustine-protocol/`, `crates/clustine-rpc/` |
| Generated game data | `tools/datagen/`, generated directories under `crates/` |

Start with the owning crate's `lib.rs`, module tree, and nearest focused tests. Use
`rg` for a symbol or behavior before reading a large module. Consult
[architecture](architecture.md) for service boundaries and the relevant
[ADR](adr/README.md) or plan for ordering-sensitive changes.
