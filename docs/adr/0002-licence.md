# ADR-0002: Licence

- Status: **Accepted**
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

The only two modern, vanilla-accurate world generation implementations in Rust are
copyleft: SteelMC's `steel-worldgen` (AGPL-3.0-or-later) and Pumpkin's `pumpkin-world`
(GPL-3.0). Every other concern (protocol, NBT, game data, infrastructure) has a permissive
path.

## Decision

Clustine is licensed under **AGPL-3.0-or-later**.

## Consequences

- An existing copyleft world generator can be reused, which removes most of parity tier T2.
  GPL-3.0 code may be combined with an AGPL-3.0 work.
- Pumpkin and SteelMC source may be consulted and adapted, with attribution, for example
  for packet layouts of the current Minecraft version.
- Anyone running a modified Clustine for players has to offer those players the source.
- Some companies will not adopt or contribute to AGPL software.
- Moving to a permissive licence later needs the consent of every contributor.
- The plugin API needs an explicit statement on how plugins may be licensed before it
  ships; see the roadmap's plugin milestone.
- Permissively licensed dependencies remain the default; a copyleft dependency is added
  only where it saves substantial work.
- The licence covers what is Clustine's. Game data that is generated from Mojang's server
  and committed is Mojang's and not under it, and code adapted from SteelMC keeps
  SteelMC's notice: [ADR-0019](0019-data-made-from-mojangs-jar.md), sections 7 and 8,
  and `NOTICE.md` at the root.
