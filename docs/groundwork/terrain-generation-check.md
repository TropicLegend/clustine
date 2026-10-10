# Terrain groundwork, checked against the clones

Checked on 2026-10-10 against the reference clones, by reading files and with `grep`,
`find`, `wc`, `git show`, `git grep`, `git ls-tree` and `git diff --stat`. Nothing was
built, run or downloaded. It checks `docs/groundwork/terrain-generation.md` (2026-10-08,
written from web pages) and is meant to be read beside it.

| Short | What | Commit |
|---|---|---|
| `S:` | a clone of `github.com/Steel-Foundation/SteelMC`, `master` (working tree) | `f8cee44`, 2026-10-09 12:10 +0200 |
| `S263:` | the same clone, `origin/26.3` (read with `git show origin/26.3:<path>`) | `885c4b3`, 2026-10-09 02:11 +0200, "Merge branch 'master' into 26.3" |
| `P:` | a clone of `github.com/Pumpkin-MC/Pumpkin`, `master` | `987b979`, 2026-10-10 |
| `X:` | a clone of `github.com/Steel-Foundation/SteelExtractor`, `master` | `e1e2695`, 2026-09-09 |

These are newer commits than the groundwork's (`f3d3451`, `558691d`, `d44f0c6`). The
clones are shallow, so history, dates of earlier commits and pull requests cannot be
checked. Line numbers of `S263:` files are those of `git show origin/26.3:<path>`.

**Not on this machine:** the vanilla data pack and the data generator's reports
(`target/datagen` did not exist when this was written; both projects git-ignore their
copy), and whichever SteelExtractor produced the 26.3 fixtures (see 3). Whether Java is
installed was not tried, as that means running something.

Verdicts: **confirmed**, **wrong**, **partly** (true with a correction), **new** (not in
the groundwork, and the plan rests on it), **not checkable** (without running or
downloading something).

---

## 1. Licences and notices

| Statement | Verdict | Evidence |
|---|---|---|
| SteelMC is AGPL-3.0-or-later; `LICENSE` ends with "Steel … Copyright (C) 2026 Alve Jeansson" and the "or (at your option) any later version" paragraph; no exception | confirmed | `S:LICENSE:632-640` (the file is the same on the branch: `git diff --stat HEAD origin/26.3 -- LICENSE` is empty); `S263:Cargo.toml:19-20` (`authors = ["Alve Jeansson"]`, `license = "AGPL-3.0-or-later"`) |
| The same licence in every crate | confirmed | all ten crates have `license.workspace = true` (`S263:steel-worldgen/Cargo.toml:5`, `steel-core/Cargo.toml:5`, `steel-registry/Cargo.toml:5`, `steel-utils/Cargo.toml:5`, `steel-math/Cargo.toml:6`, …). No crate differs |
| Whether source files carry per-file headers (was [?]) | confirmed: **they do not** | `git grep -il 'copyright\|SPDX' origin/26.3 -- '*.rs'` finds one file, `steel-core/src/command/brigadier/mod.rs:3` (a Microsoft MIT notice for the command parser, nothing to do with generation) |
| No `NOTICE` in SteelMC | confirmed for the whole tree | `git ls-tree -r --name-only origin/26.3 \| grep -i notice` is empty |
| No contributor agreement | confirmed | `S263:CONTRIBUTING.md` has no line about licences or sign-off; its only relevant line is 26, on keeping vanilla elements |
| Pumpkin is GPL version 3, the template at the end of `LICENSE` not filled in, `license = "GPL-3.0"`, nothing says "or later" | confirmed | `P:LICENSE:635,655` (`<year>  <name of author>`); `P:Cargo.toml:120`; `P:README.md:120` |
| Pumpkin's plugin crates are MIT OR Apache-2.0 and hold no generation | confirmed | `P:crates/pumpkin-plugin-api/Cargo.toml:5`, `P:crates/pumpkin-plugin-utils/Cargo.toml:5`; `P:assets/NOTICE.md` section 5 |
| `P:assets/NOTICE.md`: Mojang-derived files "are not licensed under Pumpkin's GPLv3 license"; data packs and structures "are not distributed in this repository" | partly | The text is there (section 1). The second half is not what the tree shows: `P:crates/pumpkin-data/src/generated/structure_template.rs` is committed, 22.0 MB, and holds every vanilla structure template as Rust statics (palette, packed blocks, block-entity NBT), see 5 |
| SteelExtractor is CC0-1.0 | confirmed | `X:LICENSE:1-3`; `X:src/main/resources/fabric.mod.json` (`"license": "CC0-1.0"`). The clone has no README, and nothing in it says that it is a fork of Pumpkin's Extractor (MIT by `docs/library-evaluation.md`); if it is, credit both when taking logic from it |
| SteelMC's code refers to Pumpkin as a source | not found | `git grep -i pumpkin origin/26.3 -- '*.rs'` finds only the block |
| Both projects are translations of Mojang's source | confirmed | `S263:update-minecraft-src.sh:24-31` (GitCraft, `--only-unobfuscated`, `--mappings=identity_unmapped`); `S263:README.md:106`; `P:AGENTS.md:28` ("Reviewers compare PRs against the decompiled vanilla source"), `:34` ("The 26.x jars ship without obfuscation") |

**What a file-by-file port from SteelMC needs** (AGPL sections 4 and 5; a reading, not
legal advice). Since no file carries a notice, the only notice to keep is the one in
`LICENSE`. In practice:

- A `NOTICE` in every crate that holds ported code: SteelMC, its repository, branch
  `26.3` at `885c4b3`, AGPL-3.0-or-later, "Copyright (C) 2026 Alve Jeansson" and its
  contributors.
- At the head of every ported file one sentence: which SteelMC file it was adapted from,
  at which commit, and that it was changed for Clustine and when (section 5a asks for a
  "prominent notice" of modification with a date).
- Nothing else changes: Clustine's licence is the same.
- Pumpkin's code is read, not copied (the groundwork's rule stands). SteelExtractor's
  logic may be copied.

---

## 2. What SteelMC's 26.3 branch covers, against `master`

| Statement | Verdict | Evidence |
|---|---|---|
| `master` is on 26.2 | confirmed | `S:Cargo.toml:17` (`0.15.4+mc26.2`); `S:README.md:35`; statuses `Noise`, `Surface`, `Carvers` at `S:steel-core/src/chunk/status.rs:19-23`; density in `f64` at `S:steel-worldgen/src/density/traits.rs:86` |
| The branch is on 26.3 | confirmed | `S263:Cargo.toml:17` (`0.16.0+mc26.3`). Its README still says 26.2 (`S263:README.md:35`) |
| 26.3 is not merged into `master` | confirmed, still so on 2026-10-09 | The two tips above. But `master` is merged *into the branch* a day before its tip, so the branch does not fall behind |
| The 26.3 port is 40,328 lines added and 37,092 removed | **wrong as a measure of the difference today** | `git diff --stat HEAD origin/26.3 -- '*.rs'`: 301 files, +14,720 −6,204. Restricted to `steel-worldgen`, `steel-core/src/worldgen`, `steel-core/src/chunk/light`, `steel-registry/build`: 127 files, +10,476 −3,937. (All files: 331, of which the fixtures and `blocks.json` are most of the half million changed lines.) The largest: `improved_noise.rs` ±918, `build/density/functions.rs` ±811, `feature/vanilla_collections.rs` +714, `normal_noise.rs` ±667, `transpiler/codegen_expr.rs` ±570, `blended_noise.rs` ±558 |
| One status `terrain` in place of `noise`, `surface`, `carvers` | confirmed | `S263:steel-core/src/chunk/status.rs:9-30` (Empty, StructureStarts, StructureReferences, Biomes, Terrain, Features, InitializeLight, Light, Spawn, Full); the same list in `P:assets/chunk_status.json` |
| The dependency table of section 5 | confirmed | `S263:steel-core/src/chunk/chunk_pyramid.rs:370-410`: StructureReferences and Biomes need structure starts at radius 8; Terrain needs them and biomes at radius 1, writes radius 0; Features needs them and terrain at radius 1, **writes radius 1**; Light needs InitializeLight at radius 1; Spawn needs biomes at radius 1 |
| "A finished chunk needs structure starts out to radius 10 (11 on `master`)" | not checkable; my reading is **11 on both** | The table is built by `const fn accumulate` (`chunk_pyramid.rs:112-170`). By hand: features at 1, terrain at 2, biomes at 3, and biomes need starts at 8 more. It does not matter to the plan |
| All three dimensions | confirmed | Generators `OverworldGenerator`, `NetherGenerator`, `EndGenerator` built in the test (`S263:steel-core/src/worldgen/chunk_stage_hashes.rs:1176-1196`); the fixture holds 2,500 chunks for each |
| Structures: layout in `steel-worldgen`, pieces and templates in `steel-core` | confirmed | Sixteen structure types registered at `S263:steel-worldgen/src/structure/generator.rs:1018-1033`: jigsaw, nether_fossil, fortress, end_city, woodland_mansion, ocean_monument, mineshaft, desert_pyramid, jungle_temple, swamp_hut, buried_treasure, shipwreck, igloo, ocean_ruin, stronghold, ruined_portal. A set that names another type panics at start (`generator.rs:505-516`) |
| Features: "44 files plus trees" | partly: on the branch 55 files of feature types, 8 of trees, 14 of framework | `S263:steel-core/src/worldgen/feature/` (77 files, 19,990 lines). `ConfiguredFeatureKind` has 69 kinds (`S263:steel-registry/src/feature/data.rs:52-141`) |
| Carvers | confirmed | `S263:steel-core/src/worldgen/carver/{cave,canyon,mask,mod}.rs`. `nether_cave` is a `cave` with other values now (`S263:steel-registry/build/carvers.rs:6-10`) |
| Light, sky and block | confirmed | `S263:steel-core/src/chunk/light/` (16 files, 9,268 lines), `S263:steel-core/src/worldgen/stages/light.rs` (473) |
| Heightmaps, worldgen and final kinds | confirmed | `S263:steel-core/src/chunk/heightmap.rs` (793); `status.rs:95-109` |
| Biomes | confirmed | `S263:steel-worldgen/src/biomes/` (833), `S263:steel-utils/src/climate/` (930) |
| What is stubbed or `todo!` | **new** | One `todo!`: `S263:steel-core/src/worldgen/feature/features/tree/foliage.rs:1153` (`FoliagePlacer::Poplar` in the shared "skip this leaf" test; poplars have a path of their own at `foliage.rs:185-500`, so it looks unreachable, not verified). One `panic!` for an unknown nested placement modifier (`feature/placed.rs:468`). Template pieces marked `DataMarkers` are skipped (`structure/piece_placer/template_piece.rs:43-45, 317-320`), but no structure sets that mark (only `Ignore`, `OceanRuin`, `Shipwreck`, `Igloo`, `EndCity`, `WoodlandMansion` are used). Mobs of structures that SteelMC has no entity for are skipped (`piece_placer/mod.rs:68`). The flat generator's `features` and `lakes` flags are refused (`generator/registry.rs:256-261`) |
| "Nobody states that it passes on 26.3" | still so | The branch's README repeats `master`'s claim word for word under "currently targets Minecraft 26.2" (`S263:README.md:35, 43-46`) |
| `steel-worldgen` does not fill a chunk; that is in `steel-core` | confirmed | The pipeline of one chunk is `S263:steel-core/src/worldgen/generator/vanilla.rs` (1,435); surface, carvers, features, pieces all under `steel-core/src/worldgen/` |
| The `ChunkGenerator` trait as quoted | **wrong for the branch** (it was `master`'s) | `S263:steel-core/src/worldgen/generator/mod.rs:34-110`: `create_structures(&Chunk)`, `create_biomes(&Chunk)`, `build_terrain(chunk, beardifier, neighbor_biomes)` (= `fill_from_noise`, `build_surface`, `apply_carvers`), `apply_biome_decorations(&mut WorldGenRegion)`, and besides `noise_biome`, `initial_spawn_search_origin`, `spawn_height`, `structure_generator`, `create_worldgen_region_random` |
| A write outside a step's write radius is dropped | confirmed in outline | `S263:steel-core/src/worldgen/region.rs:327` (`can_write_to_chunk`), `:515` |
| Global registries set up by `init_globals()` | confirmed | `S263:steel-core/src/worldgen/chunk_stage_hashes.rs:1073-1077` |

---

## 3. SteelMC's parity test and fixtures

| Statement | Verdict | Evidence |
|---|---|---|
| The test and its fixtures | confirmed | Test: `S263:steel-core/src/worldgen/chunk_stage_hashes.rs` (1,661 lines). Fixtures: `S263:steel-core/test_assets/chunk_stage_hashes.json` (2,298,002 bytes; 3,318,015 on `master`), `structure_starts.json` (6,129,315), `biome_hashes.json` (692,448); `S263:steel-worldgen/test_assets/noise_samples.json` (11,482) |
| Seed 13579, three dimensions, stages `minecraft:terrain`, `minecraft:features`, `minecraft:light` | confirmed | `chunk_stage_hashes.rs:96, 1081`; the fixture's first lines |
| "7,500 randomly selected chunks, 2,500 in each dimension" (was [?]) | confirmed, with what "random" means | The fixture has 7,500 chunk entries, `"chunk_count": 2500` three times. They are **25 clusters of 10×10 neighbouring chunks**, the corners drawn with Kotlin's `Random(123456)` within ±500,000 chunks (`X:…/SteelExtractor.kt:75-110`). The first cluster is at chunk (−418462, 366791) |
| What is hashed for blocks | confirmed, exactly | MD5 over the sections from bottom to top; a section whose every block is air is one byte `0`; otherwise 4,096 state ids as big-endian `i32`, in the order y, z, x (`chunk_stage_hashes.rs:302-345`; `X:…/ChunkStageHashStorage.kt:188-209`) |
| What is hashed for light | confirmed | MD5 over the lowest light section (`i32`), the count of light sections (`i32`), then for sky (byte 0) and block (byte 1), per section a byte 0 (none), 1 (empty) or 2 followed by the 2,048 bytes (`chunk_stage_hashes.rs:682-717`; `ChunkStageHashStorage.kt:129-172`) |
| Biomes, heightmaps, block entities are not in that hash | confirmed | Biomes have a test and fixture of their own: MD5 over, per section, a byte of the section's y and then the biome's name without `minecraft:` for 64 cells in the order y, z, x (`S263:steel-core/tests/biome_hashes.rs:38-84`; `X:…/extractors/BiomeHashes.kt:100-158`). Heightmaps and block entities are compared nowhere |
| Structure starts have a test | confirmed | `S263:steel-core/tests/structure_starts.rs` (971 lines): seed 13579, 2,000 chunks within 100 chunks of the origin, 69 starts, 6,510 pieces, 1,127 references; compares pieces, boxes, jigsaw state, junctions (`:419-900`) |
| A test of the noises, bit for bit | **new** | `S263:steel-worldgen/tests/noise_samples.rs:27-59`: 32-bit patterns of `NormalNoise`, the Nether's legacy noise, `BlendedNoise` and the frozen-temperature simplex noise at given points and seeds. Its name is `float_noise_matches_pre1_extractor`: the samples came from 26.3's first pre-release |
| Equality is strict | confirmed | Any differing hash is collected and the test panics (`chunk_stage_hashes.rs:1658`); no tolerance, no list of known failures |
| The test is `#[ignore]` and CI does not run it (was [I]) | confirmed | `chunk_stage_hashes.rs:881`; `S263:.github/workflows/test.yml` runs `cargo test --verbose` and nothing else. The biome and structure tests are ignored too (`biome_hashes.rs:115,132,148`; `structure_starts.rs:199`) |
| "Only supports x/z ascending generation order" | confirmed, and it is more particular than that | `chunk_stage_hashes.rs:1082-1085`. The extractor takes one cluster at a time and asks vanilla for each of its 100 chunks in ascending x then z: first to the terrain status, then to `features`, **and hashes the 100 chunks when all 100 have run their features** (`X:…/SteelExtractor.kt:431-486`, `"feature_hash_capture": "after_all_tracked_features_ready"`). The chunks around the cluster have not decorated then. So a `features` hash is **not** that of a finished chunk of a world: it is the state after exactly those 100 runs in that order. For light it then decorates the two rings around the cluster and lights the cluster and one ring (`SteelExtractor.kt:139-145, 370-384, 497-560`) |
| The fixtures were produced by "the extractor inside vanilla" | confirmed for `master`, **not checkable for 26.3** | SteelExtractor is a Fabric mod in Kotlin (`X:build.gradle:2`, fabric-loom; `X:gradle.properties`: `minecraft_version=26.2`, loader 0.19.3, loom 1.16-SNAPSHOT, Java 25). **The clone is the 26.2 extractor**: its mixin hooks `ChunkStatus.NOISE`, `SURFACE`, `CARVERS` (`X:…/mixin/ChunkStepMixin.java:22-26`), and it writes `"hashset_iteration_order": "insertion_order"` and `"biome_tie_breaker": "vanilla_rtree_chunk_local_cache"` (`X:…/extractors/ChunkStageHashes.kt:36-37`), which is `master`'s fixture header. The 26.3 fixture says `"vanilla"` and `"vanilla_thread_local_rtree"`. The extractor that wrote it is not in the clone |
| "Block for block equal to vanilla" on `master` | **partly: equal to a vanilla that the extractor changed** | On 26.2 the extractor replaces `HashSet` by `LinkedHashSet` in `TreeFeature.place` and `updateLeaves`, `FallenTreeFeature`, `VegetationPatchFeature` and the waterlogged one (`X:…/mixin/TreeFeatureMixin.java`, `FallenTreeFeatureMixin.java`, `VegetationPatchFeatureMixin.java`), forgets the biome search's thread-local last result at every chunk (`ChunkAccessBiomeCacheMixin.java`, `ClimateRTreeMixin.java`) and orders `possibleBiomes` by insertion (`BiomeSourcePossibleBiomesMixin.java`). On 26.3 the test demands `"vanilla"` hash-set order (`chunk_stage_hashes.rs:1102-1106`) and the branch has an emulation of the JDK's `HashMap` buckets, resizing and tree bins (`S263:steel-core/src/worldgen/feature/vanilla_collections.rs:15-18`, 778 lines; +714 against `master`). So the branch aims at unchanged vanilla; that it gets there is their word |
| For which game version | not checkable beyond the names | No fixture names a version. The stage names are 26.3's |
| The fixture was made with structures on | **new** | `chunk_stage_hashes.rs:98-102` (`GENERATE_STRUCTURES = true`). So `terrain` hashes include what structures do to the terrain near them, and `features` hashes include the pieces |
| Binary dumps for diffing | confirmed absent | The test reads `chunk_stage_<dimension>_<stage>_blocks.bin.gz` if present (`:361-365`); `*.gz` is git-ignored (`S263:.gitignore`). Only hashes are committed |
| **Could Clustine compute the same hashes without their code?** | **yes for biomes, terrain and noise samples; yes for features and light only with a harness that repeats the extractor's order** | The hashes are specified above and need MD5 (already a workspace dependency of Clustine, `Cargo.toml:34`). Terrain needs structure starts for chunks near a structure. Features needs "terrain for the cluster and one ring, then one run per cluster chunk in ascending x, z, then hash", which any generator can do whose feature run is a function over a 3×3 view. The biome hash has a catch: with `vanilla_thread_local_rtree` a tie between two biomes is decided by what the thread looked up last, so exact ties depend on the order of all look-ups |

---

## 4. Where SteelMC needs nightly Rust

| Statement | Verdict | Evidence |
|---|---|---|
| Pinned nightly | confirmed | `nightly-2026-08-21` in `rust-toolchain.toml` on both branches |
| It is `portable_simd` | **partly: there are four more features, none of which generation needs** | `S263:steel-worldgen/src/lib.rs:3` and `S263:steel-math/src/lib.rs:3` (`portable_simd`); `S263:steel-registry/src/lib.rs:1` and `S263:steel-utils/src/lib.rs:5` (`const_trait_impl, const_cmp, derive_const`, and `array_try_from_fn` in utils); `S263:steel-core/src/lib.rs:5` (`try_as_dyn`, used for entities only); `S263:steel/src/main.rs:2` (`thread_id_value`). The const-trait features are used in `steel-registry/src/blocks/properties.rs` (29 places) and in `steel-utils/src/{direction,axis}.rs` |
| How much generation code SIMD touches | **new** | Run time: six files of `steel-worldgen/src/noise` (`improved_noise.rs` 115 mentions, `perlin_noise.rs` 37, `blended_noise.rs` 28, `normal_noise.rs` 10, `aquifer.rs` 7, `noise_chunk.rs` 7) and three of `steel-math/src/noise_math` (82 together). Build time: the transpiler emits `Simd<f32, N>` code (`build/density/transpiler/codegen_expr.rs` 55 mentions, the emitted `use std::simd::…` at `transpiler/mod.rs:96-102`). Features, structures, carvers, surface rules and light have none |
| "SIMD has to be replaced" | partly: **most of it has a scalar twin already** | Every noise has a scalar entry beside the batched one: `ImprovedNoise::noise`, `noise_with_y_scale` (`improved_noise.rs:86, 494`) beside `noise_y_simd`, `noise_simd`; `PerlinNoise::get_value` (`perlin_noise.rs:211`) beside `get_value_simd` (`:268`); `NormalNoise::get_value` (`normal_noise.rs:224`) beside `get_value_y_simd` (`:265`); `BlendedNoise::compute` (`blended_noise.rs:145`). The transpiler has a scalar generator `gen_expr` (`codegen_expr.rs:32-660`) beside `gen_expr_simd` (`:676-`), and the trait has a scalar default for the batched call (`S263:steel-worldgen/src/density/traits.rs:195-215`). A stable port takes the scalar halves. What it must keep is the order of the `f32` operations; lane-wise SIMD and scalar IEEE arithmetic give the same bits as long as nothing is contracted into fused multiply-adds, which Rust does not do by itself |
| Other things a port must replace | **new** | `std`'s `sin`, `exp`, `ln`, `powf` are used where vanilla uses `Math.*`: the `Mth.sin` table (`S263:steel-math/src/trig.rs:30-42`), the beardifier's kernel (`S263:steel-worldgen/src/noise/beardifier.rs:45`), large dripstone (`feature/features/dripstone/large.rs:112-114`), the legacy Gaussian. In Rust these go to the platform's maths library, so two machines may differ in the last bit; Clustine's workers must agree, so it needs one implementation (the `libm` crate) and a check of the tables against the JVM |

---

## 5. How both turn Mojang's data into code

| | SteelMC (26.3 branch) | Pumpkin |
|---|---|---|
| Where the data pack comes from | A build script downloads the server jar at **every** fresh build: version manifest, size and SHA-1 checked, `data/minecraft/` unpacked into `steel-utils/build_assets/builtin_datapacks/` (`S263:steel-utils/build/build.rs:102-108, 198-236, 294-400`), git-ignored (`S263:.gitignore`). No switch for a local jar. **confirmed** | `P:crates/pumpkin-data/build.rs:26-45` downloads the 26.3 jar unless `assets/datapack/data/minecraft` exists; `PUMPKIN_MINECRAFT_SERVER_JAR` names a local one (`:76`); no checksum. **confirmed** |
| Generated Rust | Written by build scripts into git-ignored `src/generated/` (`S263:.gitignore:3-7`; `S263:steel-worldgen/build/build.rs:21-66`). **confirmed** | **Committed**: `P:crates/pumpkin-data/src/generated/`, 87 files, **85.9 MB**; made by `tools/pumpkin-codegen` (`P:AGENTS.md:59`). **confirmed; the size is new** |
| Does a build still need the jar? | Yes | **Yes, although the Rust is committed** (new): `build.rs` fetches the pack because sources `include_str!` files from it (`P:crates/pumpkin-world/src/world_info/mod.rs:242-256`) |
| Density functions, noise router | **Transpiled** to Rust functions per dimension with `proc_macro2`/`quote`: `S263:steel-worldgen/build/density/` (6,818 lines), from `worldgen/density_function/**` and `worldgen/noise_settings/*.json` (`build/density/functions.rs:297, 333-358`). Output: `vanilla_density_functions/{overworld,nether,end}.rs` (`build.rs:44-58`); its size is not known here | **Interpreted** at run time from generated static tables: `P:…/generated/noise_router.rs` (33,941 lines), interpreter in `P:crates/pumpkin-world/src/generation/noise/router/` (2,400 lines) |
| Noise parameters, noise settings | Generated constants (`build/noise_parameters.rs`, `functions.rs:1008-1300`) | `generated/noise_parameter.rs` (644), `noise_settings.rs` (252) |
| Biome parameters (which climate gives which biome) | **Not in the data pack.** Committed JSON from the extractor: `S263:steel-worldgen/build_assets/multi_noise_biome_source_parameters.json` (3.78 MB), written by `X:…/extractors/MultiNoiseBiomeParameters.kt:44-70` from `MultiNoiseBiomeSourceParameterList.knownPresets()`; turned into Rust by `build/multi_noise.rs` | `P:assets/multi_noise_biome_tree.json` (1.83 MB) from its extractor |
| Surface (material) rules | Transpiled with the density functions (`build/density/surface_rules.rs`, 541), following `material_rule` references into their registry (`functions.rs:360-445`) | `generated/material_rule.rs` (137) and an interpreter |
| Features, placed features, block state providers | Generated statics from `worldgen/feature`, `worldgen/block_state_provider`, `worldgen/placed_feature` (`S263:steel-registry/build/features/mod.rs:61, 118, 179`; 4,860 lines of build code) | `generated/configured_features_generated.rs` (12,096), `placed_features_generated.rs` (8,312) |
| Carvers | `S263:steel-registry/build/carvers.rs` (434) from `worldgen/carver` | `generated/carver.rs` (192) |
| Structures, sets, pools, processors | `S263:steel-registry/build/structure/` (2,262) and `build/features/structures.rs` (561) | `generated/structures.rs`, `template_pool.rs` (22,154), `processor_list.rs`, `structure_metadata.rs` (60,121) |
| Structure templates (`.nbt`) | **Mojang's files are embedded in the binary** with `include_bytes!` from the downloaded pack (`S263:steel-registry/build/structure/template_pools.rs:396, 438`) and parsed at first use (`S263:steel-core/src/worldgen/template/loading.rs:15-31`). **new** | **Committed as Rust statics**, 22.0 MB (`generated/structure_template.rs`). **new** |
| Loot tables | Generated (`S263:steel-registry/build/loot_tables/`, 2,157) | `generated/loot_table.rs` (44,206) |
| Block properties, shapes, light, fluid state | Committed extractor JSON `S263:steel-registry/build_assets/blocks.json` (22.8 MB), `fluids.json` | Committed `P:assets/blocks.json` (7.5 MB), `fluids.json`, `properties.json`; generated `block.rs` is 24.0 MB |

So the owner's decision (commit generated Rust) has a precedent in Pumpkin, and a warning
in it: done as plain Rust literals it is 86 MB.

---

## 6. Pumpkin on 26.3

| Statement | Verdict | Evidence |
|---|---|---|
| `master` is on 26.3, stable Rust, `rust-version = "1.96"` | confirmed | `P:Cargo.toml:117-119` (`0.2.0+26.3-26.51`); `P:rust-toolchain.toml` (`channel = "stable"`); `MC_VERSION = "26.3"` at `P:crates/pumpkin-data/build.rs:5` |
| Density functions in `f32` | confirmed | `P:crates/pumpkin-world/src/generation/noise/router/density_function/*.rs`: 608 mentions of `f32`, 48 of `f64` |
| It kept three terrain steps of its own | **new** | `P:crates/pumpkin-world/src/chunk_system/chunk_state.rs:23-45` (Noise, Surface, Carvers), all three mapped to vanilla's `Terrain` at `:91` |
| Tolerances 6,000, 7,500 and 8,000 blocks per chunk | confirmed | `P:crates/pumpkin-world/src/generation/proto_chunk_test.rs:332, 423, 502` |
| Features are printed, not asserted | confirmed | `proto_chunk_test.rs:726-789` |
| Biomes exact | confirmed | `proto_chunk_test.rs:444-476` |
| Light is compared nowhere | confirmed | No `#[test]` in `P:crates/pumpkin-world/src/lighting/` (four files, 1,720 lines) |
| A density "parity fingerprint" | **new, and not a comparison with vanilla** | `P:…/noise/router/parity_fingerprint_test.rs:22-53` hashes its own output and compares with a constant: "detects if a mass set of hashes is the same as it was previously" |
| Which version the dumps are from (was [?]) | not checkable; they look older than 26.3 | 45 files in `P:assets/tests/`, named by the old statuses (`noise_…`, `…_surface_…`, `…_carvers_…`). 26.3 has no such statuses, which would explain the tolerances; an inference |
| `generate_single_chunk` runs features for the centre only | confirmed | `P:crates/pumpkin-world/src/chunk_system/generation.rs:8-66` |

**Where Pumpkin is the better source although it is not the base** (to be read, not
copied):

- **How committed generated data can be laid out**: static tables for templates, pools,
  processor lists and metadata, and what that costs in size.
- **An interpreter for density functions** (`proto_noise_router.rs` 841,
  `chunk_density_function.rs` 424), should custom data packs ever be wanted.
- **A second explanation on stable Rust without SIMD** of the aquifer
  (`aquifer_sampler.rs`, 2,580 lines against SteelMC's 1,085) and of the structures
  (27,062 lines in 60 files against SteelMC's 27,887).
- Not for exactness, not for light, and not for 26.3's single precision: SteelMC's branch
  has that too, with scalar paths (see 4).

---

## 7. What 26.3 changed, as the two ports show it

| Groundwork (from the wiki) | Verdict | Evidence |
|---|---|---|
| Density functions and noise in single precision | confirmed | Router functions return `f32` (`S263:steel-worldgen/src/density/traits.rs:86-150`; `f64` on `master`); "the 26.3 float-based `PerlinNoise`" (`S263:…/noise/improved_noise.rs:80-86`). Coordinates stay `f64`, values are `f32` |
| New density functions `sub`, `div`, `lerp`, `slice`, `distance_to_point`, `beardifier`, `gradient`, `cache` | confirmed that the port parses them | `S263:steel-worldgen/build/density/functions.rs:47, 111, 121, 136, 172, 180, 186, 192`; also `interval_select` (152), `find_top_surface` (208), `end_outer_islands` (184) |
| `y_clamped_gradient`, `shifted_noise`, `flat_cache`, `cache_2d`, `cache_once`, `cache_all_in_cell` removed; `argument` renamed | not checkable | The port still parses all of them (`functions.rs:37, 40, 69, 164-170, 200`). Only the data pack says which the game still uses |
| `aquifers` is an object; `material_rule` in place of `surface_rule`; noise sizes gone | confirmed | `functions.rs:266-277` (barrier, fluid_level_floodedness, fluid_level_spread, lava, optional surface_level), `:279-294` (`material_rule: Option<String>`, a reference; `noise` holds `min_y` and `height` only); cell sizes are read off `interpolated` (`:930`) |
| Ore veins became a material rule | confirmed | `S263:…/density/traits.rs:34, 283-312` (`MATERIAL_ORE_VEINS_ENABLED`, values prefilled for the whole chunk before the rules run) |
| New noise format | confirmed | `S263:steel-worldgen/build/noise_parameters.rs:16-24` (`base_amplitude`, `base_octave`, `octave_count`, `normalize`, `amplitude_modifiers`) |
| `worldgen/feature`, `worldgen/carver`, `block_state_provider` registries | confirmed; `placed_feature` remains | `S263:steel-registry/build/features/mod.rs:61, 118, 179`; `build/carvers.rs:355` |
| Carvers reshaped, `nether_cave` gone | confirmed | `S263:steel-registry/build/carvers.rs:6-10` |
| Poplar trees, dappled forest, abandoned camp | confirmed | `S263:steel-registry/src/feature/data.rs:867-940`; `S263:steel-core/src/worldgen/feature/tests.rs:120-132`; `P:…/generated/structures.rs:67-69` |
| A new structure placement `dimension_origin` | **found in neither port** | No match in SteelMC's branch or in Pumpkin's sources and generated structures; SteelMC knows `RandomSpread` and `ConcentricRings` only (`S263:steel-worldgen/src/structure/placement.rs:210-225`). Either the wiki's summary was wrong or no vanilla set uses it |
| Not in the groundwork | **new** | A `block_transformer` registry (`S263:steel-registry/build/block_transformers.rs:551`, 603 lines, new on the branch); features `stepped_column_cluster` and `speleothem` (new files); vanilla's `HashSet` order now mattering to the fixtures (see 3) |

The list is still only what two ports happen to show. The data generator's output is
the ground truth and is the first thing step 0 of the plan looks at.

---

## 8. What they have that Clustine needs anyway

| Need | SteelMC 26.3 | Pumpkin | Note |
|---|---|---|---|
| Block properties per state | `X:…/extractors/Blocks.kt:373-375` writes `lightEmission`, `lightDampening`, `useShapeForLightOcclusion` per state; `:427-429` the fluid, its amount and falling; `:315-320` collision, support, outline, **occlusion**, interaction and visual shapes; `:571-626` `hasCollision`, `canOcclude`, `isAir`, `liquid`, `replaceable`, `forceSolidOn/Off` and more. Result: `blocks.json`, 22.8 MB; Rust in `steel-registry/src/blocks/` (5,320) | `assets/blocks.json` (7.5 MB), `properties.json` | **The official reports lack all of this** is still not checkable here. Every value is reachable from a plain Java program against the jar (registries only, no server), which is the plan's route |
| Fluid states | `X:…/extractors/Fluids.kt:78-177`; `S263:steel-core/src/fluid/` (1,858, behaviour) | `assets/fluids.json` | |
| Heightmaps | `S263:steel-core/src/chunk/heightmap.rs` (793) | four in a proto chunk (`generation/proto_chunk.rs`) | |
| Light engine | `S263:steel-core/src/chunk/light/` (9,268), tested against vanilla by hash | `lighting/` (1,720), untested | SteelMC's is built for incremental updates on a running server; lighting a chunk from a 3×3 from scratch is a small part of it |
| Biomes per 4×4×4 cell, palettes | `S263:steel-core/src/chunk/{section.rs,paletted_container.rs}` (1,682) | `chunk/` (6,579) | Clustine has its own palettes in `clustine-protocol` |
| Block entities in chunks | `S263:steel-core/src/worldgen/region.rs:458-620` | pending block entities as NBT | |
| **Post-processing** (new) | Generation marks positions (`region.rs:700`; aquifer and carver `carver/mod.rs:451, 474`; `generator/vanilla.rs:522`; lakes, disks, multiface growth, sculk, fallen trees). When the chunk becomes a full chunk, the fluid at each marked position ticks once and other blocks take their shape from their neighbours (`S263:steel-core/src/chunk/full_chunk/mod.rs:921-975`) | not found | Outside the hashed stages. It needs fluid and shape behaviour, which Clustine has none of. Without it, water that vanilla lets run at once stands still |
| Scheduled ticks and entities from generation (new) | `region.rs:628-692` (`add_fresh_entity`, `schedule_block_tick`, `schedule_fluid_tick`) | `spawn_mobs_for_chunk_generation` | Generation yields more than blocks: biomes, block entities, ticks, marks, entities, structure starts and references |
| Java's `HashSet` order (new) | `S263:steel-core/src/worldgen/feature/vanilla_collections.rs` (778) | not found | Needed for trees, fallen trees and vegetation patches to come out as vanilla's |
| Spawn position | `S263:steel-worldgen/src/biomes/climate_sampler.rs:108` (`find_spawn_position`), `S263:steel-core/src/world/player_spawn_finder.rs` | | |

---

## 9. Sizes: lines of Rust in SteelMC's 26.3 branch

Counted with `git grep -c '' origin/26.3 -- '*.rs'`, tests and comments included.

| Part | Lines | Files | Where |
|---|---:|---:|---|
| Random numbers (xoroshiro, legacy, positional, name hashes) | 1,508 | 6 | `steel-utils/src/random/` |
| Noise maths | 815 | 7 | `steel-math/` |
| Noise primitives (improved, perlin, simplex, normal, blended) | 2,866 | 7 | `steel-worldgen/src/noise/` |
| Noise chunk (cells, interpolation) | 499 | 1 | `…/noise/noise_chunk.rs` |
| Aquifers | 1,085 | 1 | `…/noise/aquifer.rs` |
| Ore veins | 267 | 1 | `…/noise/ore_veinifier.rs` |
| Beardifier | 287 | 1 | `…/noise/beardifier.rs` |
| End islands | 91 | 1 | `…/noise/end_islands.rs` |
| Density functions at run time | 588 | 3 | `steel-worldgen/src/density/` |
| Density transpiler | 3,873 | 10 | `steel-worldgen/build/density/transpiler/` |
| Density JSON model and driver | 2,404 | 3 | `steel-worldgen/build/density/{functions,types,mod}.rs` |
| Surface (material) rules | 1,467 | 3 | `build/density/surface_rules.rs`, `src/surface.rs`, `steel-core/src/worldgen/surface/` |
| Noise parameters, biome parameters (build) | 426 | 3 | `steel-worldgen/build/` |
| Biome source and climate | 1,763 | 7 | `steel-worldgen/src/biomes/`, `steel-utils/src/climate/` |
| Carvers | 1,773 | 5 | `steel-core/src/worldgen/carver/`, `steel-registry/build/carvers.rs` |
| Features: trees | 5,153 | 8 | `steel-core/src/worldgen/feature/features/tree/` |
| Features: the other types | 9,713 | 55 | `…/feature/features/` |
| Features: framework (placement, providers, runner, order, hash sets) | 5,124 | 14 | `…/feature/*.rs` |
| Features: data model and code generation | 6,257 | 10 | `steel-registry/{src/feature,build/features}/` |
| Value providers | 1,163 | 6 | `steel-utils/src/value_providers/` |
| Structures: layout (starts, placement, jigsaw, pieces) | 13,164 | 31 | `steel-worldgen/src/structure/` |
| Structures: placing pieces | 8,320 | 21 | `steel-core/src/worldgen/structure/` |
| Structures: templates | 3,117 | 6 | `steel-core/src/worldgen/template/` |
| Structures: data model and code generation | 3,286 | 9 | `steel-registry/{src/structure,build/structure}/` |
| Light | 10,305 | 19 | `steel-core/src/chunk/light/`, `worldgen/stages/light.rs`, `chunk_map/light_*.rs` |
| Heightmaps | 793 | 1 | `steel-core/src/chunk/heightmap.rs` |
| Generator (pipeline of one chunk) | 3,038 | 7 | `steel-core/src/worldgen/generator/` |
| Worldgen region (view of the neighbours) | 1,793 | 1 | `steel-core/src/worldgen/region.rs` |
| Stage glue (with leaf distances, 494) | 842 | 9 | `steel-core/src/worldgen/stages/` |
| **Generation and light together** | **91,580** | | |
| Block properties and shapes | 6,365 | 6 | `steel-registry/src/blocks/`, `build/blocks.rs` |
| Fluids (states and behaviour) | 2,370 | 13 | `steel-core/src/fluid/`, `steel-registry` |
| Loot tables | 4,723 | 12 | `steel-registry` |
| Biome registry code generation | 593 | 1 | `steel-registry/build/biomes.rs` |
| Parity tests | 2,860 | 5 | see 3 |

By stage of the plan: Clustine's own side has no counterpart here but light and
heightmaps (11,100, most of it machinery for a running server that Clustine does not
need); noise and density functions with the transpiler 12,600; biomes 1,800; terrain
(noise chunk, aquifers, ore veins, surface, carvers, pipeline) 8,100; features 27,400;
structures with the beardifier 28,200; the End's and the Nether's own parts are a few
hundred lines beyond their features and structures.

For comparison, Pumpkin (`find … | xargs wc -l`): noise with router and aquifer 9,286;
features 14,674 (110 files); structures 27,062 (60); carvers 1,565; surface 933; light
1,720; chunk system 4,800; `pumpkin-world` in all 79,628. Clustine today is 191,493 lines
of Rust with its tests.

---

## Corrections the groundwork needs

1. **Fact 3 and section 2, "SteelMC tests block-for-block equality with vanilla".** On
   `master` the reference is a vanilla whose hash sets and biome search the extractor
   changed. On the 26.3 branch the fixture claims unchanged vanilla, the code emulates
   Java's `HashSet`, and the extractor that made that fixture is not public in the clone.
   Nobody states that the branch passes.
2. **Section 2, what a `features` hash is.** Not a finished chunk: the state after the 100
   chunks of one cluster decorated in ascending x, z, the surroundings undecorated.
   Clustine can reproduce that protocol in a test whatever its own order is, so the
   fixture is usable; it says nothing about chunks as a world has them.
3. **Section 3, the size of the 26.3 port.** The branch differs from today's `master` by
   about 15,000 added and 6,000 removed lines of Rust, 10,500 and 3,900 of them in
   generation, and it is merged up to `master` regularly. It is nearer to `master`, and
   likelier to be merged soon, than "+40,328 −37,092" suggests.
4. **Section 4, the trait.** The quoted `ChunkGenerator` is `master`'s. The branch has
   `build_terrain` and no phases per old status.
5. **Fact 4, nightly.** `portable_simd` is the only nightly feature generation needs, and
   scalar code exists beside nearly all of it, in the noises and in the transpiler. The
   groundwork's risk "SIMD code has to become scalar code that gives the same `f32`
   results" is smaller than it reads. A risk it does not name is larger: transcendental
   functions from the platform's maths library.
6. **Section 1, data.** SteelMC's binary embeds Mojang's structure files; Pumpkin commits
   them as 22 MB of Rust, contrary to its own notice, and its build still needs the jar.
   Its committed generated Rust is 86 MB.
7. **Section 3, `dimension_origin`.** Neither port has it.
8. **Section 5, the accumulated radius** of structure starts is 11 by my reading, on both
   branches. Harmless.
9. **Section 7 and 8, what generation yields.** Besides blocks, biomes and block
   entities: positions marked for post-processing, scheduled block and fluid ticks,
   entities, structure starts and references. And vanilla's world as a player sees it is
   the features stage **plus post-processing**, in which marked fluids flow one step.
   The groundwork lists "water does not flow" under what is not generation; part of it
   is the last step of generating a chunk.
10. **Section 8, "per stage, from the game itself: most work".** Two cheaper sources were
    missed. The official server's own region files hold chunks of every status, so the
    ring of chunks it left at `minecraft:terrain` is an exact terrain reference with no
    Java written at all (an assumption about what vanilla saves, to be seen on the first
    run). And noises, random numbers, block properties and biome parameters can be asked
    of the jar's classes by a plain Java program with no server and no mod loader, since
    26.x jars are not obfuscated (`P:AGENTS.md:34`).
11. **Section 9, reason 5** ("their fixtures make tests cheap and need no Java") holds
    for noise samples, biomes and terrain. For features and light they are one seed, one
    order, one team's word, from a tool that cannot be inspected. Good as a second
    opinion, not as the gate.

## Still not checkable without running or downloading something

- That SteelMC's branch passes its own tests; that its fixtures are from the released
  26.3.
- Everything about the data pack: which density function types 26.3 still uses, whether
  the reports contain biome parameters (`reports/biome_parameters/`) or anything on
  light and collision, the sizes of the structure templates.
- The size of the Rust that SteelMC's transpiler emits (it is git-ignored).
- Why Pumpkin's tests carry tolerances.
- What the client does with missing block entities and with the heightmaps.
- How vanilla orders feature runs by itself, and whether a proto chunk is saved at
  `minecraft:terrain` (both from memory of vanilla, not from these clones).
- Whether this machine has a JDK or only a Java runtime.
