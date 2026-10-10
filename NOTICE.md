# Notices

Clustine is licensed under the GNU Affero General Public License, version 3 or later;
see [LICENSE](LICENSE). This file says what in the repository is not Clustine's own, or
is adapted from someone else's work. Why it is worded as it is, is in
[ADR-0019](docs/adr/0019-data-made-from-mojangs-jar.md), sections 7 and 8. Nothing here
is legal advice.

## Data from Minecraft

**Data from Minecraft.** The files under `crates/clustine-data/src/generated/`,
`crates/clustine-protocol/src/generated/`,
`crates/clustine-worldgen-data/src/generated/` and
`crates/clustine-worldgen-data/reference/`, and the test data under
`tools/fixtures/data/`, are made by `tools/datagen` and `tools/fixtures` from the
server of Minecraft: Java Edition 26.3 as Mojang publishes it. They hold Mojang's
data in another form: ids and names, properties and classes of blocks,
world-generation settings turned into Rust, the sizes and connection points of
structures, and values and hashes computed by the game. That data is Mojang's and
not Clustine's. **It is not under the GNU Affero General Public License, and
Clustine grants no rights to it.** The programs that make these files, and the
Rust around the data in them, are Clustine's and are under that licence. Clustine
is not affiliated with or endorsed by Mojang or Microsoft.

Of the places this paragraph names, `tools/fixtures` does not exist yet; it comes
with a later step of [the terrain plan](docs/groundwork/terrain-plan.md) and is named
here so that the paragraph covers it from its first commit.

Every generated Rust file says the same in its first line, and each generated
directory has a `NOTICE` for the packed tables in it, which can carry no comment.

## Code adapted from SteelMC

Parts of Clustine's world generation are adapted from SteelMC
(https://github.com/Steel-Foundation/SteelMC), branch `26.3` at commit
`885c4b3e60ed79862c37311780774f76806cb714`. Steel: A high-performance Minecraft
server implementation written in Rust. Copyright (C) 2026 Alve Jeansson and
contributors. Licensed under the GNU Affero General Public License, version 3 or
(at your option) any later version; see `LICENSE` at the root of this repository.
The files that were adapted say so at their head.

A crate that holds adapted code has a `NOTICE` of its own. At present that is:

- `crates/clustine-noise`

## Other projects

- **SteelExtractor** (https://github.com/Steel-Foundation/SteelExtractor, CC0, at
  commit `e1e269595b4e059abaa16d1b440b4369c55dddff`) showed which questions to ask the
  running game for values that are in its code. `tools/datagen/java/Extract.java` is
  Clustine's own program and credits it, although its licence asks for nothing.
- **Pumpkin** (https://github.com/Pumpkin-MC/Pumpkin, GPL-3.0) is read as a reference
  and not copied.
