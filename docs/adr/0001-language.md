# ADR-0001: Implementation language

- Status: **Accepted**
- Date: 2026-10-07

## Context

Clustine consists of latency-sensitive simulation workers, a connection-heavy edge tier,
a storage service, a consensus-backed coordinator and a Kubernetes operator. The language
choice affects tick latency, memory per pod, which existing Minecraft libraries can be
reused, how plugins are hosted and who is likely to contribute.

## Options

| Criterion | Rust | Kotlin/Java | Rust + Go |
|---|---|---|---|
| Tick latency predictability | No garbage collector | Collector pauses, tunable | No collector in the hot path |
| Memory per worker pod | Low | High | Low |
| Reusable Minecraft libraries | Crates from Pumpkin, Valence, Azalea, FerrumC | Minestom, Adventure and many more | Same as Rust |
| WebAssembly plugin hosting | Native (wasmtime) | Possible, awkward | Native |
| Later Bukkit bridge | Needs a JVM sidecar | In-process | Needs a JVM sidecar |
| Kubernetes operator tooling | kube-rs, good | fabric8 / Java Operator SDK, good | kubebuilder, best |
| Contributor pool | Active Rust Minecraft scene | Largest, includes plugin developers | Split across two languages |
| Toolchains to maintain | One | One | Two |

## Decision

Use **Rust for every component**, including the operator. A JVM appears only later, as the
optional Bukkit bridge.

## Consequences

- Predictable tick times and small worker pods, which matter most when running many workers.
- One toolchain, one CI setup and shared types between all services.
- Java-only contributors face a higher entry barrier; the parity matrix and small,
  well-scoped mechanics are meant to offset that.
- Bukkit plugins can never run in-process; the bridge has to cross a process boundary.
- The workspace targets stable Rust. Several Minecraft crates (Azalea, `simdnbt`) need
  nightly and are therefore kept out of the main workspace; see the
  [library evaluation](../library-evaluation.md).
