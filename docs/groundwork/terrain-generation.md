# Terrain generation: groundwork for the plan

Researched on 2026-10-08 by reading the two projects' repositories on the web, as groundwork
for the milestone after M3 (see the roadmap, "After M3"). Nothing was cloned, downloaded,
installed or built. It is not a plan: the plan is written when the owner has answered the
questions at its end, and is reviewed independently before anything is built.

## How to read this

- **[V]** means: read in the named file, through WebFetch. WebFetch passes a page through a
  small model that summarises; it refused to reproduce long files word for word. Signatures
  and constants below are as it returned them. Re-read a file before relying on a line of it.
- **[I]** means: inferred from what was read. **[?]** means: not verified.
- Licence statements are a reading of the files, not legal advice.

Commits looked at:

| Short name | Repository and commit | Date of commit |
|---|---|---|
| `S:` | `https://github.com/Steel-Foundation/SteelMC/blob/f3d345146b9849aaf57123667ca3bbd18912f962/` (branch `master`) | 2026-10-08 |
| `S263:` | `https://github.com/Steel-Foundation/SteelMC/blob/558691d627b97a3c1d4341a7d32c01b7d19ab07c/` (branch `26.3`) | 2026-10-07 |
| `P:` | `https://github.com/Pumpkin-MC/Pumpkin/blob/d44f0c6a44e3b28bc581cb6962adf75d1f555c83/` (branch `master`) | 2026-10-08 |

Could not reach: `https://www.minecraft.net/article/minecraft-java-edition-26-3` (timed
out); the README of `Steel-Foundation/SteelExtractor` (404 on `raw…/master/README.md`; the
repository page gave only its description); SteelMC's documentation site (not tried). The
26.3 changes below come from `https://minecraft.wiki/w/Java_Edition_26.3` and from the two
projects' own 26.3 ports.

One correction to the brief: there is no `crates/clustine-worldgen/`. The package
`clustine-worldgen` lives in `services/worldgen/` (118 lines, `FlatGenerator`).

---

## The five facts that shape everything

1. **Minecraft 26.3 rewrote much of world generation.** Density functions are evaluated in
   single precision. Noise settings, density function types, carvers, features and the
   noise format were renamed or restructured. The chunk statuses `noise`, `surface` and
   `carvers` became one status `terrain`. A generator for 26.2 does not produce 26.3
   terrain. See section 3.
2. **SteelMC's `master` is on 26.2. Its 26.3 work is on a branch, not merged.** Pumpkin's
   `master` is on 26.3 since 2026-09-18.
3. **SteelMC tests block-for-block equality with vanilla; Pumpkin does not.** SteelMC's
   test demands equal hashes of every block state per stage, and of light. Pumpkin's tests
   allow 6,000 to 8,000 differing blocks per chunk for terrain and do not assert on
   features at all.
4. **SteelMC needs nightly Rust (`portable_simd`) and its chunk pipeline lives inside its
   server crate.** Pumpkin builds on stable Rust 1.96 and has a synchronous entry point
   that generates one chunk without a running server. Neither is published on crates.io.
5. **Both download Mojang's server jar in a build script** to get the vanilla data pack.
   Clustine's ADR-0004 says a build needs nothing but cargo, and that the data pack is
   never committed.

---

## 1. Licences

### SteelMC

- **Licence: AGPL-3.0-or-later.** [V] `S:LICENSE` is the AGPL version 3 text (34,520 bytes)
  with the notice "Steel: A high-performance Minecraft server implementation written in
  Rust. Copyright (C) 2026 Alve Jeansson". No additional permission or exception. [V]
  `S:Cargo.toml` has `license = "AGPL-3.0-or-later"`; the README says "GNU Affero General
  Public License v3.0 or later".
- **Into Clustine (AGPL-3.0-or-later): the same licence.** As a dependency and as copied
  source, both are allowed. Copied code keeps its copyright notice; changed files say that
  they were changed and when (AGPL section 5a); the licence text stays with the work.
  Practical form: a `NOTICE` naming the repository, the commit, the licence and the
  copyright holder, and a header line in each derived file.
- I did not check whether their source files carry per-file headers. [?]
- No contributor licence agreement is mentioned in `S:CONTRIBUTING.md`. [V]

### Pumpkin

- **Licence: GPL version 3.** [V] `P:LICENSE` is the GPL version 3 text; the template at
  its end is not filled in, so no copyright holder is named. [V] `P:Cargo.toml` has
  `license = "GPL-3.0"`, an old identifier that does not say "only" or "or later". The
  README says "Licensed under the GNU General Public License v3.0 (GPLv3)". Nothing says
  "or later". **Treat it as GPL-3.0-only.** [I]
- The plugin crates (`pumpkin-plugin-api`, `pumpkin-plugin-wit`, `pumpkin-plugin-utils`)
  are MIT OR Apache-2.0; they contain no generation. [V] `P:AGENTS.md`
- **Into Clustine: allowed, with two consequences.** Section 13 of both licences permits
  combining GPLv3 and AGPLv3 work into one work. The Pumpkin part stays under GPL version
  3. (a) The combined work can then only be conveyed under version 3: Clustine's "or
  later" stops being usable for as long as Pumpkin code is in it. (b) Files taken from
  Pumpkin cannot be relabelled AGPL-3.0-or-later. They are best kept in a crate of their
  own with `license = "GPL-3.0-only"` and a notice. This holds for a dependency and for
  copied source alike.
- The library evaluation says Pumpkin was MIT until 2026-02-01. Not re-checked. [?] Code
  from then would target 1.21.x and is of no use for 26.3.

### Data derived from the game

| | SteelMC | Pumpkin |
|---|---|---|
| Vanilla data pack (worldgen JSON, structure `.nbt`) | Not committed since 2026-06-14 (commit `8a8a85a`, "Don't track buitin_datapack, download it buildtime instead"). [V] `S:steel-utils/build/build.rs` reads Mojang's version manifest, downloads the server jar into memory, checks size and SHA-1, and unpacks `data/minecraft/` into `steel-utils/build_assets/builtin_datapacks/` (git-ignored). No offline switch and no way to name a local jar; it skips the download if the unpacked data is there. A Nix build pins the jar instead | Not committed. [V] `P:crates/pumpkin-data/build.rs` downloads the 26.3 server jar (no checksum) and unpacks `data/` into `assets/datapack/` (git-ignored). `PUMPKIN_MINECRAFT_SERVER_JAR` names a local jar instead; an existing `assets/datapack/data/minecraft` skips it |
| Data extracted from the running game | Committed JSON from **SteelExtractor**, a Fabric mod (CC0-1.0, a fork of Pumpkin's Extractor): `steel-registry/build_assets/` (26 files, 23.4 MB, of which `blocks.json` 20.1 MB), `steel-worldgen/build_assets/multi_noise_biome_source_parameters.json` (3.8 MB) [V] | Committed JSON in `assets/` (`blocks.json` 7.5 MB, `multi_noise_biome_tree.json` 1.8 MB, and others) [V] |
| Rust generated from that data | Git-ignored; generated by build scripts at every build [V] `S:.gitignore` | **Committed** in `crates/pumpkin-data/src/generated/`, regenerated with `cargo run -p pumpkin-codegen` [V] `P:AGENTS.md` |
| Vanilla reference output for tests | Committed: `steel-core/test_assets/` (hashes and structure starts, 10.1 MB) [V] | Committed: `assets/tests/` (44 files, 98 MB, whole chunk dumps) [V] |
| What they say about it | Nothing found. No `NOTICE` seen [V for the top of the tree only] | [V] `P:assets/NOTICE.md`: the Mojang-derived files are "not licensed under Pumpkin's GPLv3 license" and remain Mojang's; the data pack and structures "are not stored in the repository" |

For Clustine this means: their licences cover their code. They do not make Mojang's data
free to redistribute. Whatever Clustine takes from their `build_assets/` or `assets/` is
Mojang's data that someone else extracted; ADR-0004 as written forbids committing it.

One more thing the owner should know: both projects are written by translating Mojang's
code. SteelMC's contributors generate the vanilla source with `update-minecraft-src.sh`
(GitCraft, flags `--only-unobfuscated`, `--mappings=identity_unmapped`) and are told to
verify against it. [V] Pumpkin's `AGENTS.md` says "Reviewers compare PRs against the
decompiled vanilla source, so work from that source too". [V] Their Rust is their own
expression, but its structure follows Mojang's. Clustine inherits that provenance with
whatever it takes. This is a question for the owner, not something I can settle.

---

## 2. What their generation covers

Vanilla 26.3's chunk statuses, as Pumpkin's extractor dumped them from the game: [V]
`P:assets/chunk_status.json`

```
empty, structure_starts, structure_references, biomes, terrain, features,
initialize_light, light, spawn, full
```

### SteelMC (`master`, Minecraft 26.2)

| Part | State | Where (under `S:`) |
|---|---|---|
| Biome source (multi-noise, climate) | present | `steel-worldgen/src/biomes/biome_source.rs`, `climate_sampler.rs`, `nether_climate_sampler.rs`; `steel-utils/src/climate/parameter_list.rs` |
| Density functions, noise router | present; **transpiled to Rust at build time** from the data pack JSON | `steel-worldgen/build/density/` (transpiler, 250 KB), `steel-worldgen/src/density/`, `src/noise/` (perlin, simplex, blended, normal noise) |
| Noise settings | present, as generated constants | generated; `steel-worldgen/build/noise_parameters.rs` |
| Aquifers | present | `steel-worldgen/src/noise/aquifer.rs` (37 KB) |
| Ore veins | present | `steel-worldgen/src/noise/ore_veinifier.rs` |
| Surface rules | present, transpiled | `steel-worldgen/build/density/surface_rules.rs`, `steel-worldgen/src/surface.rs`, `steel-core/src/worldgen/surface/mod.rs` |
| Carvers | present | `steel-core/src/worldgen/carver/` (`cave.rs`, `canyon.rs`, `mask.rs`) |
| Features | present: 44 feature files plus trees (8 files), dripstone, ores, lakes, geodes, monster rooms | `steel-core/src/worldgen/feature/` |
| Structures | present: placement, starts, jigsaw, stronghold, mineshaft, fortress, monument, mansion, end city, igloo, ruined portal, shipwreck, ocean ruin, pyramids, temple, swamp hut, nether fossil | layout in `steel-worldgen/src/structure/`; placing blocks in `steel-core/src/worldgen/structure/piece_placer/` and `template/` |
| Heightmaps | present, worldgen and final kinds | `steel-core/src/chunk/heightmap.rs` |
| Lighting | present, sky and block | `steel-core/src/chunk/light/` (about 400 KB), `steel-core/src/worldgen/stages/light.rs` |
| Other dimensions | Nether and End present | `NetherGenerator`, `EndGenerator`; `steel-worldgen/src/noise/end_islands.rs` |

I read the directory listings and the generator, pyramid, region and test files. I did not
read each feature or structure file. [V for the listing, ? for completeness of each]

**Parity claim.** [V] `S:README.md`: "Its parity suite compares 7,500 randomly selected
chunks with a reproducible vanilla reference: 2,500 in each dimension." "All tested chunks
match block for block." "Entity spawning is not included".

**How it is tested.** [V] `S:steel-core/src/worldgen/chunk_stage_hashes.rs`

- The test `chunk_stage_hashes` compares against `steel-core/test_assets/chunk_stage_hashes.json`
  (3.3 MB), which "the extractor" produced inside vanilla. Seed 13579. Three dimensions.
- Stages on `master`: `noise`, `surface`, `carvers`, `features`, `light`.
- Per chunk and stage it takes the MD5 of every section's 4,096 block state ids. Light has
  a hash of its own over sky and block light of every section.
- Equality is strict: any differing hash panics. No tolerance, no list of known failures.
- **Not hashed there:** biomes (a separate test, `S:steel-core/tests/biome_hashes.rs`),
  heightmaps, block entities. Structure starts have their own test
  (`S:steel-core/tests/structure_starts.rs`, fixture 6.1 MB).
- The test is `#[ignore]` ("takes too long … run with --release"). `test.yml` is 316 bytes,
  so CI probably does not run it. [I] I could not run it. The claim is theirs.
- It notes "only supports x/z ascending generation order". Features of neighbouring chunks
  overlap, so the result depends on the order in which chunks are decorated. See section 5.

### Pumpkin (`master`, Minecraft 26.3)

Everything is in `P:crates/pumpkin-world/src/generation/` (214 files).

| Part | State | Where (under `P:crates/pumpkin-world/src/`) |
|---|---|---|
| Biome source | present | `biome/multi_noise.rs`, `generation/biome.rs` |
| Density functions, noise router | present; **interpreted** at run time from generated tables; `f32` | `generation/noise/router/` (`proto_noise_router.rs`, `chunk_noise_router.rs`, `density_function/`) |
| Noise settings | present | `pumpkin-data` (`noise_settings`, generated) |
| Aquifers | present | `generation/noise/aquifer_sampler.rs` (101 KB) |
| Ore veins | present | `generation/noise/ore_sampler.rs` |
| Surface rules | present | `generation/surface/`, `pumpkin-data` `material_rule` |
| Carvers | present | `generation/carver/` |
| Features | present: 57 feature files plus trees (38 files), coral, dripstone, sculk | `generation/feature/` |
| Structures | present: jigsaw, stronghold, fortress, mansion, monument, end city, mineshaft, pyramids, igloo, shipwreck and more | `generation/structure/` |
| Old-chunk blending | present | `generation/blender/` |
| Heightmaps | present (four in a proto chunk, three in a finished one) | `generation/proto_chunk.rs` |
| Lighting | present, sky and block, their own flood fill | `lighting/engine.rs` |
| Other dimensions | Nether and End have tests | `generation/proto_chunk_test.rs` |

**No parity claim.** The README's list has "Chunk Generation" unchecked, though the
tracking issue #36 was closed as complete on 2026-08-09 with every item ticked. [V]

**How it is tested.** [V] `P:crates/pumpkin-world/src/generation/proto_chunk_test.rs`

- Against dumps of vanilla chunks in `assets/tests/`. Seeds 0 and 13579, a handful of
  chunk positions and 8×8 grids.
- **Tolerances:** `let allowed_mismatches = 6000;` for noise, `7500` for surface, `8000`
  for carvers, out of 98,304 blocks of an overworld chunk. No comment says why.
- **Features:** `verify_grid_features` prints the number of mismatches and asserts nothing.
- Biomes: exact. Structure starts and bounding boxes: exact.
- **Light is compared nowhere.** Heightmaps only for saving and loading.
- The dumps do not say which Minecraft version they are from. [?] They may predate the
  26.3 port, which would explain tolerances; I do not know.
- Fixes for parity were still arriving this week (PR #3953, a draft, "sample tree foliage
  before placement work", 2026-10-08). Several parity fixes from September were closed
  without being merged (#3297, #3299, #3233, #3314); whether they landed some other way I
  do not know. [?]

---

## 3. Versions, and what 26.3 changed

| | Targets | Evidence |
|---|---|---|
| Clustine | 26.3, protocol 777, data version 5023 | `crates/clustine-data/src/generated/version.rs` |
| SteelMC `master` | **26.2** | [V] `S:Cargo.toml` version `0.15.4+mc26.2`; README "currently targets Minecraft 26.2" |
| SteelMC branch `26.3` | **26.3** | [V] `S263:Cargo.toml` version `0.16.0+mc26.3`. PR #656 "26.3 worldgen" (+40,328 −37,092, 147 files) merged into the branch on 2026-09-25; test fixtures regenerated then and on 2026-09-29 for the final 26.3. The branch's README still says 26.2. Not merged to `master`; last commit 2026-10-07 |
| Pumpkin `master` | **26.3** | [V] `P:Cargo.toml` version `0.2.0+26.3-26.51`; `lib.rs` has `CURRENT_MC_VERSION = "26.3"`. Port merged 2026-09-18 (PR #3495, 10,694 files) |

On the SteelMC branch the test's stages are `minecraft:terrain`, `minecraft:features`,
`minecraft:light`, still with strict equality. [V] `S263:steel-core/src/worldgen/chunk_stage_hashes.rs`
Nobody states that it passes on 26.3. [?]

**What 26.3 changed in world generation** (minecraft.wiki's page for 26.3, read in parts;
the wiki confirms release 2026-09-15, protocol 777, data version 5023, data pack format
121.0, up from 107.1):

- "Noise and density functions are evaluated in single precision instead of double
  precision", in every intermediate step. This changes results everywhere.
- Density functions: `argument` → `input`, `argument1/2` → `left/right`,
  `y_clamped_gradient` → `gradient`, `shifted_noise` removed (now `noise` with shifts),
  `cache_2d`, `flat_cache`, `cache_all_in_cell` removed, `cache_once` → `cache`,
  `interpolated` takes the cell sizes; many added (`sub`, `div`, `lerp`, `beardifier`,
  `slice`, `distance_to_point`, …).
- Noise settings: `surface_rule` → `material_rule`; `aquifers_enabled` → an `aquifers`
  object; `ore_veins_enabled` → an `ore_veins` list; `preliminary_surface_level` →
  `chunk_surface_level`, no longer interpolated; `final_density` no longer adds the
  beardifier by itself; `noise.size_horizontal/size_vertical` removed.
- Noises: new format (`base_octave`, `octave_count`, `amplitude_modifiers`, `normalize`).
- Registries: `worldgen/configured_feature` → `worldgen/feature`; `configured_carver` →
  `carver`; new `material_rule`, `material_condition`, `block_state_provider`.
- Carvers reshaped; `nether_cave` removed. Ore veins became a material rule
  (`minecraft:ore_vein`).
- Features removed, renamed and added; new placement modifiers; a new biome (dappled
  forest), poplar trees, a new structure (abandoned camp) and a new structure placement
  (`dimension_origin`).
- One status `terrain` in place of `noise`, `surface`, `carvers` (seen in Pumpkin's dump
  of the game's statuses and in SteelMC's port; not in the part of the wiki I read).

I am not sure this list is complete. The data generator's output for 26.3 is the ground
truth for the data. It is not on this machine (`target/datagen/` does not exist here), so
I could not look at it.

**What has to be bridged:** from SteelMC `master`, everything above; the branch has done
it. From Pumpkin `master`, nothing in version terms.

A point that matters for option (c): the data generator gives the *data* (the density
function graph, noise parameters, material rules, feature and structure configurations,
structure templates). It does not give the *algorithms*: the noise implementations, the
random number generators and how they are seeded by name, the aquifer, the carvers, each
feature type, the hard-coded structure pieces (stronghold, mineshaft, fortress, monument),
the light engine. Those exist only as code: Mojang's, SteelMC's or Pumpkin's.

---

## 4. Structure of their code

### SteelMC

Crates: `steel` → `steel-login` → `steel-core` → `steel-worldgen` → `steel-protocol` →
`steel-macros` → `steel-registry` → `steel-utils`/`steel-math` → `steel-crypto`. [V]
`S:AGENTS.md`: "Orchestration: `steel-core/src/worldgen/`", "Algorithms and data:
`steel-worldgen/src/`", "Codegen: `steel-worldgen/build/`".

- **`steel-worldgen`** (`S:steel-worldgen/`): 53 source files, about 700 KB, plus a
  250 KB build script. Noise, density functions, biome sources, aquifer, ore veins,
  beardifier, structure layout. Depends on `steel-registry`, `steel-utils`, `steel-math`,
  `rayon`, `glam`, `wincode`, `sha2`. Has `#![feature(portable_simd)]`. [V] `S:steel-worldgen/src/lib.rs`
- **It does not fill a chunk.** The pipeline, surface application, carvers, every feature,
  structure pieces, heightmaps and light are in `steel-core/src/worldgen/` (126 files,
  about 1.4 MB) and `steel-core/src/chunk/`. `steel-core` is the whole server. The library
  evaluation's "fairly isolated" holds for the noise layer only.
- **Interface.** [V] `S:steel-core/src/worldgen/generator/mod.rs`, trait `ChunkGenerator`:

  ```rust
  fn create_structures(&self, chunk: &Chunk);
  fn create_biomes(&self, chunk: &Chunk);
  fn fill_from_noise(&self, chunk: GenerationChunk<'_, NoisePhase>, beardifier: Option<&Beardifier>);
  fn build_surface(&self, chunk: GenerationChunk<'_, SurfacePhase>, neighbor_biomes: &dyn Fn(IVec3) -> u16);
  fn apply_carvers(&self, chunk: GenerationChunk<'_, CarversPhase>);
  fn apply_biome_decorations(&self, region: &mut WorldGenRegion<'_>);
  ```

  Constructor [V] `S:steel-core/src/worldgen/generator/vanilla.rs`:
  `VanillaGenerator::new(world_path: Option<&Path>, biome_source: BiomeSourceKind, seed: u64, thread_pool: &rayon::ThreadPool)`.
  Chunks are mutated in place. Their type is steel-core's `Chunk` with locked sections.
- **Neighbours.** [V] `S:steel-core/src/worldgen/region.rs`: `WorldGenRegion` borrows a
  `StaticCache2D<Arc<ChunkHolder>>` and a `WorldGenContext` that leads to the `World`. It
  offers block get and set, block entities, heightmaps, biomes, scheduled ticks, entities.
  A write outside the step's write radius is logged and dropped.
- **Global state.** A global `REGISTRY`, set up by `init_globals()`; `BLOCK_BEHAVIORS`,
  `BLOCK_ENTITIES`. [V] `vanilla.rs`, `region.rs`, the test.
- **Async.** `AGENTS.md`: "Don't use async unless you need disk or network I/O". The
  generation calls are synchronous; the server drives them from a tokio runtime with rayon
  pools. [V] `S:steel-core/benches/worldgen.rs`. `steel-utils` depends on `tokio`.
- **Toolchain.** [V] `S:rust-toolchain.toml`: `nightly-2026-08-21`. On the 26.3 branch
  the generated density code uses `Simd::<f32, N>` throughout (PR #656). [V]
- **Not published.** crates.io has no `steel-worldgen`. [V] `https://crates.io/api/v1/crates?q=steel-worldgen` (empty)

**(a) As a dependency: not workable inside Clustine's workspace.** It needs nightly; the
part that fills chunks is the whole server; its build downloads the jar; the 26.3 code is
on an unmerged branch. It could only run as a separate program built with nightly that
speaks Clustine's messages, which breaks the single binary.

**(b) As vendored source: large.** Roughly 3 MB of Rust to adapt (the noise layer, the
transpiler, features, carvers, structures, light, heightmaps, random numbers, the block
and biome registries behind them). SIMD has to be replaced or put behind a stable crate.
Their `Chunk`, `ChunkHolder` and `World` have to be replaced by Clustine's types. Block
state ids are vanilla's `u16` in both, so states map one to one once versions agree.

### Pumpkin

Crates under `P:crates/`. `pumpkin-world` holds generation **and** chunk formats, chunk
I/O, the chunk system, lighting, ticks and the level. Depends on `pumpkin-data`,
`pumpkin-util`, `pumpkin-nbt`, `pumpkin-config`, `tokio` (multi-thread runtime, fs,
signal), `rayon`, `crossbeam`, `dashmap` and more. [V] `P:crates/pumpkin-world/Cargo.toml`

- **Interface.** [V] `P:crates/pumpkin-world/src/chunk_system/generation.rs`:

  ```rust
  generate_single_chunk(generator: &WorldGenerator, block_registry: &dyn WorldPortalExt,
                        chunk_x: i32, chunk_z: i32, target_stage: StagedChunkEnum) -> Chunk
  ```

  It builds a square `Cache` of fresh `ProtoChunk`s around the position, runs the stages in
  order and returns the centre. No `Level`, no tokio, no threads in that path.
  `VanillaGenerator::new(seed, dimension)` [V] `P:…/generation/generator/mod.rs`.
- **Stages** [V] `P:…/chunk_system/chunk_state.rs`: Empty, Biomes, StructureStart,
  StructureReferences, Noise, Surface, Carvers, Features, Lighting, Spawn, Full. Surface
  reads biomes at radius 1. Features, Lighting and Spawn read and write at radius 1.
- **Chunk representation.** [V] `P:…/generation/proto_chunk.rs`: `ProtoChunk` has a flat
  `Vec<BlockStateId>`, biomes at 4×4×4 resolution, four heightmaps, light per section,
  pending block entities as NBT, structure starts. A finished chunk has paletted sections,
  three heightmaps and light.
- **What the caller must supply:** `WorldPortalExt` with `can_place_at`, `mirror`,
  `rotate`, `spawn_mobs_for_chunk_generation`. Their benchmark uses a stub whose
  `can_place_at` always returns true. [V] `P:crates/pumpkin-world/benches/chunk_gen.rs`
  The real one lives in the server and knows each block's rules.
- **A trap in `generate_single_chunk`.** [V] `P:…/chunk_system/generation_cache.rs`:
  `advance` runs Features for the centre chunk only. What the centre's features write into
  neighbours is thrown away with the cache, and what neighbours' features would write into
  the centre never happens. Used naively, **every tree that crosses a chunk border is cut
  in half.** Their server avoids this with its scheduler, which moves proto chunks in and
  out of caches. An adapter has to do the same.
- **Toolchain.** Stable; `rust-version = "1.96"`. Clustine says 1.85; the compiler on this
  machine is 1.99.0.
- **Not published.** crates.io has no `pumpkin-world`. [V]

**(a) As a git dependency behind an adapter: workable, days not weeks.** Pin a commit; set
`PUMPKIN_MINECRAFT_SERVER_JAR` to the jar `cargo datagen` already caches, or the build
downloads it; raise Clustine's minimum Rust; write a neighbourhood driver over their
`Cache`; convert their finished chunk into Clustine's. Costs: a heavy dependency tree, an
internal interface with no stability promise, the GPL-3.0 part, and terrain that is not
vanilla's block for block.

**(b) As vendored source: large, like SteelMC's,** minus the SIMD problem, plus the
licence boundary, and on a base whose parity is not asserted.

---

## 5. Performance and determinism

**A chunk is not a function of (seed, position) alone at the features stage.** Vanilla
generates in stages that reach into neighbours. SteelMC mirrors vanilla's table. [V]
`S263:steel-core/src/chunk/chunk_pyramid.rs` (26.3 branch):

| Status | Needs around it | Writes blocks within |
|---|---|---|
| structure_starts | – | – |
| structure_references | structure starts, radius 8 | – |
| biomes | structure starts, radius 8 | – |
| terrain | structure starts radius 8, biomes radius 1 | own chunk |
| features | structure starts radius 8, terrain radius 1 | **radius 1** |
| initialize_light | – | – |
| light | initialize_light radius 1 | own chunk |
| spawn | biomes radius 1 | – |
| full | – | – |

Accumulated, a finished chunk needs: light data at radius 1, terrain at radius 2, biomes
at radius 3, structure starts out to radius 10. [V] (11 on `master`.)

- Up to and including **terrain**, a chunk is a pure function of the seed and its
  position. Structure starts are too.
- **Features** of a chunk read and write the 3×3 chunks around it. A chunk is finished
  only when all eight neighbours have run their features. Where features of two chunks
  touch, the result depends on which chunk went first. Vanilla has this too. SteelMC's
  test fixes the order to ascending x and z to match the extractor's run. [V]
- So "block for block" at the features stage means: for a fixed order of generation.

**Scheduling.**

- SteelMC: the pyramid above; tasks through `ChunkHolder::apply_step` on a tokio runtime
  with rayon pools. [V] `S:steel-core/benches/worldgen.rs`, `S:steel-core/src/chunk/chunk_scheduler.rs` (listing only)
- Pumpkin: a thread "Schedule" owns a graph of (chunk, stage) tasks; generation runs on a
  rayon pool of `(cpus / 2).clamp(2, 16)` threads, at most twice that many tasks in
  flight; disk I/O on tokio. The source does not claim that the result is independent of
  the order of scheduling. [V] `P:crates/pumpkin-world/src/chunk_system/schedule.rs`

**Speed.**

- SteelMC [V] README: "generated a fresh 10,201-chunk Overworld area in a median of 3.98
  seconds" on a Ryzen 9 9950X (16 cores). That is about 2,560 chunks a second for the
  machine. The number of threads is not stated; at 16 to 32 threads it is 6 to 12 ms of
  thread time per chunk. [I] That is with nightly, SIMD and link-time optimisation.
- Pumpkin: benchmarks exist (`P:crates/pumpkin-world/benches/`); no numbers in the files I
  read. [?]
- For scale: a player joining at Clustine's default view distance of 8 needs 289 finished
  chunks, hence terrain for 441.

---

## 6. Activity and stability

| | SteelMC | Pumpkin |
|---|---|---|
| Size | 887 commits, 750 stars | 3,020 commits, 12.2k stars |
| Commits on `master` | 10 in the 9 days to 2026-10-08 | 10 in the 3 days to 2026-10-08 |
| Generation code | `steel-core/src/worldgen` touched by 22 commits from 2026-08-07 to 2026-10-05; `steel-worldgen` by 4 in the same time | `crates/pumpkin-world/src/generation` touched by 20 commits from 2026-09-03 to 2026-10-08 |
| Large upheavals lately | Three chunk types merged into one (2026-08-02); global registries reworked (2026-08-07); nightly bumped (2026-08-23); "remove vanilla comments" (2026-10-03); the 26.3 port (40k lines each way), still on a branch | The 26.3 port (2026-09-18); the server split into core and host crates (2026-10-02); "many generation fixes" (2026-09-09) |
| New Minecraft version | 26.2 arrived 2026-06-16; 26.3 not on `master` three weeks after release | 26.3 merged three days after release |

Both move fast and reshape their internals freely. Neither publishes a crate or promises
an interface.

- **Following upstream as a pinned dependency** is realistic for Pumpkin only, and means
  re-doing the adapter at each bump.
- **Following upstream in vendored, adapted source** is not realistic for either. It
  becomes a one-time port, with single fixes carried over by hand.
- Each new Minecraft version can change generation. 26.3 was a large one.

---

## 7. Where generation would run in Clustine

### Today

- `clustine_world::ChunkGenerator` (`crates/clustine-world/src/chunk.rs`):
  `fn generate(&self, position: ChunkPos) -> Chunk` and `fn settings(&self) -> String`. Its
  comment requires determinism, "because unmodified chunks are regenerated instead of
  being persisted".
- The store calls it on its one chunk thread, in `Job::Load`
  (`stored.unwrap_or_else(|| self.generator.generate(position))`) and in `apply`, which
  folds block changes into chunks (`services/worldstore/src/chunks.rs`).
- `docs/world-format.md`: "Only changes are stored." The world's `meta` records
  `generator.settings()` and refuses another generator.
- A `Chunk` is sections of block states with **one biome per section**. No light, no
  heightmaps, no block entities.
- The edge builds the chunk packet (`services/edge/src/encode.rs`): three heightmaps that
  are all "highest block that is not air"; sky light per column from
  `clustine_world::sky_light`; no block light; `block_entities: Vec::new()`; one biome per
  section; a fluid count that knows only water and lava blocks.
- `clustine-data` knows a block's range of state ids and its default state. It has no
  properties, no light data, no shapes.

### Staged generation and regions

**The store is the right place, and regions need not know.** Neighbours of a chunk that
is being generated are, to the generator, proto chunks in the store. No worker is asked
for anything. The rule that makes this safe is vanilla's own: a chunk is handed out only
when finished, and a finished chunk is never written by generation again, because all
eight neighbours have already run their features. So generation never writes into a chunk
that a region holds or a player has changed.

What this does to the interface, in two shapes the plan has to choose between:

- **Shape A: staged and stateful, as vanilla.** The store (or a worldgen service behind
  it) keeps proto chunks, advances them by status, and decorates each chunk once. Exact
  parity with vanilla for a fixed order is possible. But a chunk is then no longer a
  function of its position: it depends on what was decorated before. So "regenerate what
  nobody changed" stops working. Finished chunks, and proto chunks that have received a
  neighbour's features, have to be stored, durably, before anything depending on them is
  shown. That is a new kind of state in the store, with its own crash questions.
- **Shape B: pure, by a scratch neighbourhood.** `generate(position)` stays a pure
  function. Inside, it computes terrain for the 5×5 around the chunk, runs the features
  of each of the 3×3 chunks on a pristine copy (each sees terrain and its own features
  only), and merges what falls into the centre in a fixed order. Every chunk's features
  are then the same whoever asks, so there are no seams, nothing extra to store, and
  restarts change nothing. It differs from vanilla only where features of different
  chunks would have interacted (a tree growing into a neighbour's tree; snow on a
  neighbour's leaves). Cost: nine times the feature work when cold, about once with a
  cache of intermediate results.

Both need the same things from Clustine:

1. **Generation off the store's chunk thread.** One chunk costs milliseconds; a join
   needs hundreds. Today that would stall saves and checkpoints, which share the thread.
   It needs a pool, and the promise "a load that follows a save finds what was saved" has
   to survive. This is the `worldgen` service of the architecture, at last with work to do.
2. **A seed**, in `settings()` and on the command line. And a **generator version** in
   `settings()`: with "only changes are stored", any fix to the generator changes the
   untouched chunks of an existing world next to the touched ones. Either worlds refuse a
   newer generator, or generated chunks are stored once shown. This argues for storing
   them whichever shape is chosen.
3. **A spawn position** from the terrain. Today it is `FlatGenerator::surface_y()`
   (`bin/clustine/src/lib.rs`).

### What changes where

| Place | Today | Needed |
|---|---|---|
| `ChunkGenerator` | `generate(position) -> Chunk`, no seed | A seed and a version in `settings()`. Shape B keeps the signature. Shape A needs stages and a view of neighbours |
| `Chunk`, `Section` (`clustine-world`) | one `Biome` per section | 64 biome cells per section (4×4×4). Block entities. Perhaps light and heightmaps, if they are stored and not derived |
| Section and manifest encodings (`clustine-format`) | biome `u16` per section; "all air in the air biome" needs no file | A biome container per section. The air shortcut must cope with air sections whose biomes differ. A status, if proto chunks are stored. A format version bump |
| Store (`services/worldstore`) | generates on the chunk thread; stores changed chunks only; path `manifests/overworld/` | A generation pool or service; a cache of intermediate stages; for shape A durable proto chunks; later one directory per dimension |
| Messages (`clustine-rpc`) | a `Chunk` as postcard; a mixed section is 4,096 state ids | Real terrain makes most sections mixed. Expect on the order of 100 KB per chunk from store to worker and worker to edge, where a flat chunk is almost nothing. Measure; probably send palettes |
| Light (`clustine-world/src/light.rs`) | sky light per column, no block light | A real engine: sky and block light, spreading sideways, across chunk borders (needs the 3×3), with per-state opacity and emission |
| Edge (`services/edge/src/encode.rs`) | one heightmap used three times; biome `Single`; no block entities | Three real heightmaps; biome palettes; block entities; fluid counts that know waterlogged and water plants |
| `clustine-data`, `cargo datagen` | names and id ranges | Block properties per state (in the official `reports/blocks.json`). Light emission, opacity, "blocks motion", fluid state per block state (**not** in the official reports [?]; needs an extractor or their extracted data). The worldgen data itself |
| Oracle (`tools/botswarm/src/oracle.rs`) | `level-type=minecraft\:flat`, `generate-structures=false` | A normal world with a given seed |

### What the client needs that a flat world lets Clustine skip

- **Light that is right.** The client does not light chunks itself; it shows what the
  server sends. With today's rule everything under a leaf is at light 0: forests would be
  black underneath, overhangs and cave mouths too. This is seen in the first minute.
- **Block light.** Lava, glow lichen, magma and the like light caves in vanilla.
- **Biomes per 4×4×4 cell.** Grass, leaf and water colours, the sky and fog, the F3
  screen. One biome per section gives 16-block steps of colour at biome borders.
- **Three different heightmaps.** `MOTION_BLOCKING` counts water, `WORLD_SURFACE` counts
  everything but air, `MOTION_BLOCKING_NO_LEAVES` skips leaves. As far as I know the
  client uses them for where rain and snow fall. [?] Clustine has no weather yet, so this
  shows late; the comparison with the official server shows it at once.
- **Block entities.** Chests, beds, signs, banners and spawners from structures and
  monster rooms. As far as I know the client draws these only if the chunk packet lists
  them, so they would be invisible. [?] Not needed before features.
- **Block states with properties.** Log axes, snowy grass, leaf distance, water levels.
  The generator has to name states by property.
- **Fluid counts** per section that include waterlogged blocks and plants that are water.
- **A spawn point** that is not inside a mountain or an ocean.

And what terrain shows up that is not generation at all, but would be noticed within
minutes in a real world: water and lava do not flow when a block next to them is broken,
sand and gravel do not fall, leaves do not decay, and players pass through blocks (the
parity matrix: positions are "taken from the client without checking speed or
collisions"; the client itself stops its player at blocks). The plan should say whether
these belong to this milestone.

---

## 8. Options

How a generated chunk can be compared with the official server, for every option:

- **Through the protocol (exists in outline).** `flat_chunk_matches_the_official_server`
  in `services/edge/src/encode.rs` already compares blocks, biomes by name, heightmaps,
  block entities and light of the chunks a bot receives. Give the oracle
  `level-type=minecraft\:normal` and a `level-seed`, join, compare the chunks around the
  spawn point with Clustine's for the same seed. It compares finished chunks only, in the
  order vanilla happened to generate them. Blocks that tick right after generation (water
  that starts to flow, grass, snow) may differ; whether the server can be frozen first
  through its console I have not tried. [?]
- **Light and heightmaps without any generator.** Decode the official server's chunks of
  a normal world, run Clustine's light engine and heightmaps over *their* blocks, compare
  with *their* light and heightmaps. This tests the engine before a line of terrain
  exists.
- **Per stage, with SteelMC's fixtures.** `S263:steel-core/test_assets/chunk_stage_hashes.json`
  holds MD5 hashes per chunk for `terrain`, `features` and `light` of 26.3, seed 13579,
  three dimensions. Clustine can compute the same hash and compare, with no Java. Hashes
  are not Mojang's data. That they are right is SteelMC's word. [?]
- **Per stage, from the game itself.** A small Java program run against the server jar
  (as `cargo datagen` runs the data generator) that generates chunks to a status and
  dumps or hashes them. This is what their extractors do as Fabric mods. Most work, most
  certain.

### (a) SteelMC: depend on it, or vendor it

- **Depend:** not possible in the workspace (nightly, server-wide coupling, jar download
  in the build, 26.3 unmerged). As a separate nightly-built generation process: about 8
  to 12 steps (a harness like their test's, the conversion, the process and its messages,
  the build). Loses the single binary. Risk: the harness sits on their internals, which
  changed shape three times since August.
- **Vendor wholesale:** 30 or more steps; see (c), which is this with the pipeline
  written for Clustine.
- **Owner sees:** with the separate process, vanilla terrain with structures and light
  within a couple of weeks, on the owner's machine only after installing a nightly
  toolchain (ask first).
- **Parity:** theirs, block for block if their claim holds for the branch.

### (b) Pumpkin: depend on it, or vendor it

- **Depend:** about 6 to 10 steps. (1) Raise the minimum Rust; add the git dependency at
  a pinned commit in a crate of its own, GPL-3.0-only. (2) Point its build at the cached
  jar. (3) A neighbourhood driver so that neighbours' features land. (4) Implement
  `WorldPortalExt`. (5) Convert their chunk, with biomes, heightmaps, light and block
  entities, to Clustine's; this needs the chunk type, format and edge changes of section
  7 anyway. (6) Generation off the chunk thread. (7) Oracle comparison, reported as a
  count of differing blocks.
- **Risks:** not vanilla's terrain block for block, and no way for Clustine to fix that
  except upstream; the stub `can_place_at` puts plants where they cannot live; their
  light is untested against vanilla; a large dependency tree (tokio with a runtime,
  dashmap, their NBT and config crates) inside the store; the build needs the jar, so
  ADR-0004's "building needs nothing but cargo" ends; the licence pins the whole to
  version 3; every bump of the pin can break the adapter.
- **Owner sees:** real-looking terrain, trees, caves, villages and light, soonest of all
  options: one to two weeks.
- **Vendor:** as large as (c), on a base without asserted parity. Not worth it.

### (c) Write it for Clustine, from the official data, with their code as the source of the algorithms

- A new crate (say `crates/clustine-terrain`) on stable Rust and Clustine's types. Data
  from `cargo datagen` out of the official data pack. Algorithms ported from **SteelMC's
  26.3 branch** (same licence, strict tests, and its noise layer is already a crate of
  its own), with attribution. Pumpkin's code read as a second explanation, not copied,
  to keep one licence. The pipeline, the neighbourhood and the storage are Clustine's own.
- **Steps, roughly:** groundwork on Clustine's side 8 to 12 (section 7's table: chunk
  type, formats, light engine, heightmaps, datagen, pool, wire size, seed, spawn);
  terrain 10 to 15 (random numbers, noises, density functions, biome source, aquifers,
  material rules, carvers); features 15 to 25 (some 50 feature types, trees, placement
  modifiers; each is separate work and suits subagents); structures 15 to 20. These are
  guesses. SteelMC took from March to June 2026 for the same on 26.1/26.2 (noise
  2026-03-11, surface 03-14, structure starts 05-11, carvers 05-13, features 05-20, light
  06-24, by their commit log), then about three weeks for 26.3.
- **Risks:** the largest effort. Their 26.3 branch is three weeks old and may still
  change. Their SIMD code has to become scalar code that gives the same `f32` results, bit
  for bit. Decisions on Mojang's data (below). The provenance question of section 1.
- **Parity:** checked stage by stage against SteelMC's hashes and against the official
  server; Clustine owns every difference and can fix it.

### (d) A staged path

This is an order of work, and fits (c) best. With 26.3's single `terrain` status the
natural stages are:

| Stage | Contents | What the owner sees on joining | Check |
|---|---|---|---|
| 0 | Clustine's side only: biome cells, real light, real heightmaps, block properties, generation pool, seed, wire size | Nothing new: the flat world, now with exact light under what players build | The flat comparison still passes; light and heightmaps computed over the official server's normal-world blocks equal the official ones |
| 1 | Biomes and terrain: noise, aquifers, ore veins, material rules, carvers; spawn | Mountains, oceans, rivers, caves, lava lakes below, grass, sand, snow caps, biome colours and names as vanilla for the seed. No trees, plants or ores | SteelMC's `minecraft:terrain` hashes; side by side with a vanilla world of the same seed |
| 2 | Features, block entities for monster rooms | Forests, flowers, ores, lakes, geodes, snow and ice | `minecraft:features` hashes for a fixed order; the protocol comparison |
| 3 | Structures: starts, references, pieces, templates | Villages, mineshafts, strongholds; empty chests and no villagers until Clustine has inventories and mobs | SteelMC's structure-start fixture; the protocol comparison with structures on |
| 4 | Nether and End | Needs dimensions in Clustine first | The same hashes, other dimensions |

Stage 1 is not split into "shape" and "surface": 26.3 does not split it, and a world of
bare stone would look wrong to anyone.

---

## 9. Recommendation

**Build it as Clustine's own crate on stable Rust, in the stages of (d), by porting the
algorithms from SteelMC's 26.3 branch with attribution, fed by the official data through
`cargo datagen`, and checked stage by stage against SteelMC's hash fixtures and against
the official server. Depend on neither project.**

Reasons:

1. **The stated goal is exact.** The parity matrix's first row for world generation reads
   "Overworld terrain (bit-exact for a seed)". Only SteelMC tests for that. Pumpkin's
   tests allow thousands of wrong blocks per chunk and do not check features or light.
2. **Neither can be a dependency on Clustine's terms.** SteelMC: nightly, SIMD, the
   pipeline is the server, 26.3 unmerged. Pumpkin: possible, but it brings a second
   licence that pins the whole work to version 3, a runtime-heavy dependency into the
   store, a jar download into every build, and terrain Clustine cannot correct.
3. **Most of the work is Clustine's own in every option.** Biome cells, a real light
   engine, heightmaps, block properties, block entities, generation off the chunk thread,
   the neighbourhood, storage and message sizes: none of it comes from either project,
   because none of them stores or distributes a world as Clustine does.
4. **The same licence.** SteelMC's code can be adapted file by file with a notice and
   nothing else changes for Clustine.
5. **Their fixtures make "tests from the specification by someone who did not write the
   code" cheap:** per-stage hashes for 26.3 exist and need no Java.
6. **Both upstreams move too fast to follow.** A port is done once; later Minecraft
   versions are bridged from the data generator's diff and their ports as reference.

What I would have the plan decide first, because the rest depends on it: shape A or B of
section 7. I lean to **B (pure, by a scratch neighbourhood)** for stages 1 and 2: it keeps
Clustine's determinism and "regenerate or store, it is the same chunk", it needs no new
durable state, and through the terrain stage it is exact. Its cost is that overlapping
features of neighbouring chunks will not match vanilla's order-dependent result, so the
`features` hashes will not all match. If the owner wants those exact as well, it has to be
A, and A changes what the store promises. The independent reviewer should attack exactly
this point.

An alternative the owner may prefer, stated fairly: **Pumpkin as a pinned dependency for
a quick first look** (option (b), one to two weeks), kept behind `ChunkGenerator` and
thrown away later. It would show terrain soonest and exercise all of Clustine's side
early. I do not recommend it as the base: it defers the real work, and what the owner
would judge by is terrain that is not vanilla's.

### Open questions for the owner

1. **The goal.** Block for block equal to vanilla for a seed, as the parity matrix says,
   or "looks like Minecraft"? And where vanilla itself depends on the order of generation
   (features across chunk borders): is a fixed rule of Clustine's own acceptable there?
2. **Mojang's worldgen data.** ADR-0004 lets only ids and names be committed. Generation
   needs the density function graph, noise and biome parameters, material rules, feature
   and structure configurations. May Rust generated from them be committed (Pumpkin does;
   SteelMC commits the biome parameters)? If not, either every build needs the jar, or
   the server reads the data pack from the operator's own jar when it starts. Structure
   templates are Mojang's files in any case: a server with structures needs the jar or a
   cache of it at run time. What then of container images?
3. **Their code and Mojang's code.** Both projects are translations of Mojang's source.
   Is porting from SteelMC acceptable on that footing? May Mojang's own source (not
   obfuscated since 1.21.11, going by SteelMC's script) be read where their port is
   unclear?
4. **Dependency or own code**, and is following their upstream wanted at all? Is a
   GPL-3.0-only part, which pins Clustine to version 3, acceptable?
5. **A quick first look with Pumpkin**, to be removed again: wanted or not?
6. **Scope of the first milestone.** Overworld only? Structures in or out? (They bring
   block entities, loot and inhabitants that Clustine does not have.) Which stage of (d)
   ends the milestone?
7. **Stored or regenerated.** May generated chunks be stored once shown, giving up "only
   changes are stored"? Otherwise every later fix to the generator alters existing worlds
   or locks them out.
8. **Block data the official reports lack** (light emission, opacity, what blocks motion,
   fluid states): write a small Java program that asks the server jar, run by `cargo
   datagen`; or take SteelExtractor's or Pumpkin's extracted JSON; or keep a table by
   hand?
9. **What terrain exposes that is not generation:** water that does not flow, sand that
   does not fall, leaves, walking through blocks. In this milestone, or named as known
   limits?
10. **Toolchain.** Stay on stable Rust (my assumption)? Raising the minimum from 1.85 is
    needed only if Pumpkin is used.
11. **Seed and presets.** A `--seed` option and one default seed? Are large biomes,
    amplified and custom data packs out of scope (transpiling the data, as SteelMC does,
    rules custom data packs out; interpreting it, as Pumpkin does, keeps them possible)?
12. **Speed to aim for** on the owner's six processors: how long may a first join into
    fresh terrain take?

### Not verified

- That SteelMC's parity test passes, on `master` or on the 26.3 branch. I ran nothing.
- That SteelMC's fixture holds the 7,500 chunks the README speaks of.
- Why Pumpkin's tests carry tolerances, and how far its terrain really is from vanilla.
- The completeness of the list of 26.3 changes; the official changelog could not be read.
- That the official data generator's reports lack light and collision data, and that
  they include the biome parameter list (I believe `reports/biome_parameters/` exists;
  the output is not on this machine).
- What the client does with heightmaps and with block entities missing from a chunk
  packet. A real client has to show it.
- Every figure for effort. They are guesses to be replaced by the plan.
