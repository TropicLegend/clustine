# ADR-0002: Licence

- Status: **Open** (no licence chosen yet; the repository contains no licence file)
- Date: 2026-10-07

## Context

Clustine is meant to be an open-source project that attracts contributors and is run by
server operators, including commercial hosts. The licence also decides which existing
libraries can be reused: copyleft dependencies cannot go into a permissively licensed
project, while permissive dependencies fit either choice. The findings per library are in
the [library evaluation](../library-evaluation.md).

## Options

| | Apache-2.0 OR MIT | AGPL-3.0 |
|---|---|---|
| Convention | The Rust ecosystem default | Common for server software that wants changes shared |
| Hosts running modified versions | No obligation to publish changes | Must offer their source to users of the service |
| Reusable dependencies | Permissive only | Permissive and GPL-family |
| Adoption by companies | Easy | Often restricted by policy |
| Plugins | Any licence | Linking questions need an explicit plugin exception |
| Changing later | Relicensing to copyleft is easy | Relicensing to permissive needs every contributor's consent |

## What the library evaluation adds

The only two modern, vanilla-accurate world generation implementations in Rust are
copyleft: SteelMC's `steel-worldgen` (AGPL-3.0-or-later) and Pumpkin's `pumpkin-world`
(GPL-3.0). Pumpkin as a whole moved from MIT to GPL-3.0 in February 2026.

- Choosing AGPL-3.0 allows reusing one of them, which removes most of parity tier T2.
- Choosing Apache-2.0/MIT means writing vanilla world generation from scratch.

Every other concern (protocol, NBT, game data, infrastructure) has a permissive path.

## Decision

None yet.

## Consequences of leaving this open

- Without a licence file nobody else may legally use or contribute to the code.
- No external contribution should be merged before this is decided, since changing the
  licence afterwards needs every contributor's agreement.
- Dependencies under copyleft licences must not be added until this is decided.
