# Agent verification workflow

Pick checks based on the change. Begin with the narrowest useful feedback, then widen
the scope for shared contracts, concurrency, persistence, or protocol behavior. All
commands run from the repository root.

## Fast local loop

For any Rust edit:

```bash
cargo fmt --all --check
cargo check -p <crate> --all-targets --locked
```

Run the relevant crate's tests, or filter to a focused test while iterating:

```bash
cargo test -p <crate> --locked
cargo test -p <crate> --locked <test_name_filter>
```

For an integration test in `bin/clustine/tests/<name>.rs`:

```bash
cargo test -p clustine --test <name> --locked
```

Do not treat a filtered test run as coverage of the rest of the crate. Before finishing,
run the full affected crate tests when practical and the workspace checks for changes to
shared contracts or behavior across crate boundaries.

## Choose broader checks

| Change | Add these checks |
|---|---|
| Rust API, shared protocol/RPC types, or cross-service behavior | `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --locked` |
| Edge packet behavior or protocol encoding | Relevant `clustine` integration test; official-server comparison via `tools/oracles.sh <target-filter>` when applicable |
| Region ordering, durability, merge/split, handover, or crash recovery | Relevant simulation/worker/worldstore tests and the full workspace test suite; inspect the corresponding ADR |
| Generated game data or datagen inputs | `cargo datagen --check` with the pinned jar and JDK available |
| Kubernetes manifests, image, or deployment behavior | `deploy/kind/test.sh` |
| Any change before integration | `tools/check.sh` for the full local suite; CI also runs the cluster and datagen workflows when their path filters match |

The full workspace suite is intentionally expensive. `tools/check.sh` records logs and
runs the workspace and pinned end-to-end suites concurrently; see its output for the log
directory. Official-server tests are ignored by default and can hang without the
per-target limits in `tools/oracles.sh`. Do not run them as one unbounded Cargo command.

Tests should wait for a state or message rather than sleep. Do not pipe a check through
`grep` or `tail` unless the shell preserves its exit status. State which checks ran and
which were skipped in the handoff.
