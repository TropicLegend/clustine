# Phase S: the structures' blocks, planned

Draft of 2026-10-15, revised after an independent review the same day (section 9). Phase S of the
[terrain plan](terrain-plan.md) (S1 to S3), after phase F ([its plan](f-plan.md)) was
built and switched. Phase B ([its plan](b-plan.md)) goes beside it; where the two
meet is said in section 6.

## 1. Where things stand

- **Layouts are done** (T7). All 52 structures of the overworld, the Nether and the
  End start where the game starts them, with every piece, judged against the game's
  own classes (`crates/clustine-structures/tests/game_structures.rs`, `game_bent.rs`).
  The ground they bend (the beardifier) is in the terrain and judged with it.
- **Each piece carries what it needs to be placed**: `StructurePiecePayload` is
  `Jigsaw` (pool element, position, rotation, liquids), `Template` (template,
  rotation, mirror, pivot, processors, liquids, marker handling, adjustments,
  post-processing) or `Procedural` (with vanilla's state per piece: chests placed,
  height, traps, a monument's rooms, a stronghold's doors, a mineshaft's kind). The
  design is SteelMC's, so its placers fit the payloads almost one to one.
- **A template can be placed** (F5a): `TemplateLibrary` reads the game's templates
  from its jar, all eleven processors with their rule tests, position tests and
  block-entity modifiers, waterlogging and the fluid ticks of liquids, block
  entities with their loot seeds; rotation and mirroring by block class. Fossils and
  the sulfur caves' templates use it.
- **Nothing places a structure's block.** `FeatureStages::starts` gives no starts and
  `terrain` no references; `decorate` places features only. The scheduler's plumbing
  for starts, references and the state of pieces (`Neighbourhood::starts`, `piece`,
  `set_piece`, `Placing::Piece`) is there and unused.
- **The judge runs with structures off on both sides** (`generate-structures=false`,
  `TerrainGenerator::without_structures`).

## 2. What the game does, which the port follows

In `ChunkGenerator.applyBiomeDecoration`, for each of the eleven steps in turn: first
every structure whose step it is, in the registry's order, each with the decoration
seed of its index and the step (`setFeatureSeed(decoration, index, step)`). The index
is the structure's place among **all** the registry's structures of that step, the
Nether's and the End's included and those with no start here counted too
(SteelMC-26.3 `runner.rs:296-301`); the features' indices of the step are counted
apart and stay as they are; for each
such structure, the starts that the centre chunk references, in the order of
`StructureManager.startsForStructure` (the references' `LongOpenHashSet` order, which
`long_set_order` already gives); of each start, every piece whose box meets the
writable box of the centre chunk, `placeInChunk`, then the start's `afterPlace` (two
structures have one, by the jar: the desert pyramid turns some of the sand its pieces
noted into suspicious sand with its own draws, and the woodland mansion fills the
ground below its floor with cobblestone). Then the features of the step, as
now.

The decoration seed is set again per structure and per feature, but the run's
**region random** (`WorldGenRegion.getRandom`) is one stream for the whole run: the
desert pyramid draws from it, as do the mansion's allays and a dropped mob's
`finalizeSpawn`, and features draw from it after (`simple_block.rs:174`). So the
placers are handed the region random that `run.rs` now gives features alone, and
draw what the game draws. Features also read the blocks structures left, so a
feature beside a village can come out differently once the village stands. Structures with entities (villagers, illagers, a witch, a guardian, an
item frame) make them in `placeInChunk` through `addFreshEntityWithPassengers` on the
`WorldGenRegion`; the port draws what the game draws and drops the entity, as phase
F did, named in the parity matrix.

## 3. The judge

`game_features.rs` judges with structures on:

- The game side gets `generate-structures=true`; the Clustine side a
  `TerrainGenerator::new` and `FeatureStages` that hands the scheduler the real starts
  and references.
- Until every structure is built, a chunk is **judged** only if every piece whose box
  meets its 5×5 neighbourhood is of a built kind: a chunk's blocks come from the runs
  of its 3×3, and each of those reads its own 3×3 (the dependency cone of F); a
  template's shape updates at its box's skin and a feature element's tree also reach
  past a piece's box, by less than a chunk; others are
  **counted** and listed by the structure that kept them out, so the count falls as
  batches arrive and nothing is hidden at the end. Masking by the pieces' boxes
  instead was considered and rejected: a feature beside an unbuilt piece reads the
  missing blocks and differs outside the box.
- The chooser (`areas_where_every_biome_of_the_overworld_stands`) gets a twin,
  `areas_where_every_structure_stands`: for each of the overworld's 34 structures an
  area of 24×24 chunks around one start, on seeds 13579 and 12345. The Nether's and
  the End's wait for phase D.
- Two judges as in F: the area sweep (both seeds' biome areas and structure areas,
  every chunk equal block for block, with block entities, ticks, marks and
  heightmaps), and the spawn judge (ten seeds) again, since a structure can stand on
  the spawn column.

## 4. Steps

Sizes are of the reference ports (SteelMC-26.3 / Pumpkin), as a guide.

| # | Scope | Who | Verified by |
|---|---|---|---|
| S0 | The judge with structures on, the twin chooser, judged/counted chunks as in section 3; `FeatureStages` passes starts and references, and the scheduler's `Start` carries the start as T7 made it (pieces with their payloads, shared, made once per start where starts are made and cached with them, so that a run never lays a jigsaw out again); `decorate` runs each step's structures before its features with the region random handed through, with **every placer a stub that places nothing and says it is unbuilt**; per structure, what of a piece's state outlives a run is checked against `PieceState` (`shift_y`, settled, flags) | me | The F sweep still passes where no piece reaches; the counts list every structure; a test that `decorate` calls the placers in the game's order (registry order with the index of section 2, references' order, pieces in order, `afterPlace`), from the specification, by someone else |
| S1 | Shared parts: the processor lists emitted by datagen (`worldgen/processor_list`, now three hand-written fossil lists); data markers; pool elements (single, legacy, feature, list, empty) with `structure_block_ignore`, `jigsaw_replacement`, gravity, `known_shape`, the element's own liquid settings; the shape updates a template does as it is placed (`updateShapeAtEdge` for the box's skin, and `Block.updateFromNeighbourShapes` where `known_shape` is false) | me | Unit tests against SteelMC's and the game's processor outputs; the jigsaw structures below |
| S2a | Jigsaw structures: villages (five), pillager outpost, ancient city, trail ruins, trial chambers, the 18 abandoned camps (bastions with D) | subagent | Sweep areas of these structures equal |
| S2b | Template families: shipwreck (two), igloo, ocean ruins (two), ruined portals (the overworld's five), with their processors, markers and post-processing; buried treasure | subagent | as S2a |
| S2c | Mineshafts (two) and the stronghold | subagent | as S2a (1,138 + 920) |
| S2d | Ocean monument; desert pyramid, jungle temple, swamp hut | subagent | as S2a (1,797; 734 + 370 + 125) |
| S2e | Woodland mansion (its template markers and post-processing; its `afterPlace`, which no reference port has, written into the brief from the jar's class by me) | subagent | as S2a |
| S3 | The switch: the generator's **identity** changes (`terrain;features 5;structures 1;...`), so that older seeded worlds are refused with the message of F6 (a raised version alone is accepted by the store, which refuses only an older generator), the spawn judge, the first-view and flight measurements again, docs | me | The full sweep with no structure counted; the spawn judge 10 of 10; measurements within the speed answers of the terrain plan (question 9) |

The Nether's fortress, bastion and fossil and the End's city wait for phase D and
are counted until then (their layouts are built).

Subagents work on files of their own (one placer module each under
`crates/clustine-features/src/structures/`: the placers need the features' `Level`,
its region random and the placed features of jigsaw elements, and
`clustine-features` already depends on `clustine-structures`), from a brief that states the payload, the
interface of S0 and S1, the areas that judge them and how to run the judge; they do
not start Java or the official server: I run the judge on what comes back.

## 5. Entities, chests and what a player meets

- Entities are dropped (villagers, iron golems, illagers, the witch and her cat,
  guardians and elder guardians, item frames, armour stands): Clustine has no mobs
  yet. A village without villagers is what a player will see; named.
- Chests and other containers keep their loot table and seed as the game saves them;
  they do not open (W6's known limit), and are filled by the loot table when opening
  comes.
- Spawners keep their mob and are sent with it, as the monster rooms' are.
- Water in monuments, ocean ruins and shipwrecks is placed as the game places it, with
  its ticks and marks; phase B settles them.

## 6. Where S meets B

B2 (the shape and survival rules moved into one crate, `clustine-blocks`) comes
before S1, which needs `updateShape` for a template's skin; S0 does not wait for it.

Structures leave marks and fluid ticks (waterlogged blocks, the monument's water, a
village's water in a field), and a template's blocks are placed with
`UPDATE_CLIENTS` only (no neighbour updates in generation, as in F). The world's
chunks then carry them, and phase B's B6 settles them when a region first takes the
chunk. Nothing in S waits for B.

## 7. Questions

None are the owner's, by the rules already agreed: templates are read from the
operator's jar (terrain plan question 2, answer a), older worlds are refused with a
message, and structure entities are dropped as features' were. If the owner wants
the villages' people before mobs come, that is a milestone of its own.

## 8. Risks

| Risk | Answer |
|---|---|
| A structure's piece reaches further than its box (a feature element's tree, a template's jigsaw neighbour) | The 3×3 rule of section 3 covers what reaches the centre; a difference outside it shows in the sweep |
| Vanilla's order of starts in `startsForStructure` differs from the references' set order in some case | It is the order `long_set_order` reproduces for references and was judged in T7; a difference shows as two structures overlapping differently |
| Templates' block entities with data Clustine does not model (a lectern's book, a decorated pot's sherds, a vault's state, a trial spawner's configuration) | Saved as the game saves them (the template's NBT through the processors), sent as `sent(entity)` allows; what the client needs is checked with the official comparisons |
| Trial chambers' vaults and trial spawners are block entities with large data | as above |

## 9. What the review changed

An independent review against the code and the reference ports, 2026-10-15:

| # | Finding | Answer |
|---|---|---|
| 1 | Raising `FEATURES_VERSION` does not refuse older worlds; the store refuses only an older generator | The identity changes (S3) |
| 2 | The judge's 3×3 is too narrow: a chunk depends on its neighbours' runs | 5×5, F's cone (section 3) |
| 3 | The region random is shared by structures and features | Handed to the placers (section 2, S0) |
| 4 | The placers have no source for the payloads; a jigsaw laid out again per run is costly | Starts carried whole and cached (S0) |
| 5 | The seed index counts all of a step's structures | Said (section 2), tested in S0 |
| 6 | No reference port of the mansion's `afterPlace` | From the jar into the brief (S2e) |
| 7 | S1 and B2 touch the same rules | B2 first (section 6) |

## 10. As built

- **S0** (2026-10-15): as planned. The judge's count of chunks kept out by unbuilt
  structures proved too weak: mineshafts reach under most areas. So the judge also
  **turns whole sets off on both sides** when none of their structures is built (in the
  game a data pack leaves those structures no biome; in Clustine their starts are
  dropped), and counts only what a partly built set keeps out. Only whole sets, since
  within a set the game chooses among the structures that can start; the judge
  refuses to turn off a set another set keeps its distance from.
- **S1, the part jigsaw structures need, and S2a** (2026-10-16): the game's processor
  lists emitted by datagen; pool elements (single, legacy, feature, list, empty) with
  the game's processors in its order, the shape known, no marks (the game places them
  with the flags 18, which hold `UPDATE_KNOWN_SHAPE`); block entities saved again as
  each type's own code saves them (`clustine-structures/src/resave.rs`: containers,
  furnaces, brewing stands, vaults, signs, hoppers, banners, sculk sensors); `block_rot`
  asked of the block as the processors before left it, so a jigsaw replaced by its
  final state can rot. Templates placed without a known shape (fossils, the template
  feature) take the features' `updateShape`, so gravel beside a fossil schedules its
  tick; paths and farmland schedule theirs under something solid. Judged: every
  jigsaw structure's area on seed 13579 (27 areas: villages of five kinds, the
  pillager outpost, the ancient city, trail ruins, trial chambers, the abandoned
  camps) equal block for block with block entities, ticks and marks; on seed 12345
  26 of 27, the other one block: an acacia fence beside a tree in a savanna camp that
  the game joins to the tree's edge, since the features' `updateShape` has no fences
  yet; B2, which makes one set of rules of the features' and the simulation's, brings
  them.
