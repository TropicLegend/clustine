# N7l: the animals and the structures' entities made with each chunk

Draft of 2026-10-21, for the owner and an independent reviewer before anything is built.
The game's rules are those of `n7-brief.md` (cited as B: B 8.4 is its section 8.4, B 8.0.6
finding 6 of section 8.0), read from the 26.3 jar; the dumps are in
`/home/claude/reference/n7-javap/` under the classes' full names (`NaturalSpawner.txt` below
is `net.minecraft.world.level.NaturalSpawner.txt`), and line numbers are of the `javap`
output, as the brief gives them. The decisions already agreed are `n7-plan.md` (cited as P),
above all P 7.6, 8.6, 9.5 and 10.2 and the plan as it stands, P 11.2 ("Generated entities:
keys from the chunk with the top bit set; taken once (a blocks file means taken, by the
round's rule); remakes keep stored entities") and P 11.5 ("N7l made with the chunk (the
pool's step delegated, the store and keys mine), the format raised"). This plan says how
Clustine builds them, and decides what P leaves open, saying why.

Where things stand (`main` at `f4a3334`): the first half of N7 is built; of the second,
N7h (tamed and ridden animals, the horse family, riding) and the water half of N7i (fish,
squids, dolphins, the turtle). Not built: the wild animals of N7i (armadillo, camel, panda,
fox, polar bear, goat, frog), the brains of N7j, and N7k (golems, villagers, zombie
villagers, witches, illagers, elder guardians, drowned, allays as bodies). Nor is the part of
N7a2 that P 11.4 gave the store's generated-chunk rules and the key layout: no generated key
exists, `EntityIds::BLOCK_COUNT` is still `u32::MAX` (`clustine-world/src/position.rs` 159),
and the store's note writes blocks only (`worldstore/src/chunks.rs` 395–406). N7l takes
both over.

## 1. What a player sees when N7l is done

### 1.1 Animals already in a new world

A seeded world's chunks are made with the animals the game makes with them
(`NaturalSpawner.spawnMobsForChunkGeneration`, B 8.4), only of the category `creature`
(B 8.0.5): no fish, squid, bats or monsters with the chunk.

- **Which, by biome**: B 8.9's table, read from the biomes' data. Plains have sheep, pigs,
  chickens and cows in fours, horses in herds of two to six and the odd donkey; savannas
  armadillos in twos and threes besides, and the plateau llamas and wolf packs of four to
  eight; forests wolves, flower forests rabbits, the dappled forest rabbits and foxes;
  taigas wolves, rabbits and foxes; windswept hills llamas; jungles parrots in ones and twos
  and a rare panda, bamboo jungles many pandas; swamps frogs of two to five, mangrove swamps
  frogs alone; badlands armadillos, wooded badlands wolves; deserts rabbits and the odd camel;
  snowy plains and ice spikes rabbits and polar bears; snowy slopes rabbits and goats, the
  peaks goats alone; groves wolves, rabbits and foxes; meadows donkeys, rabbits and sheep;
  cherry groves pigs, rabbits and sheep; mushroom fields mooshrooms of four to eight; beaches
  turtles; frozen oceans a polar bear on the ice. Rivers, the other oceans, stony peaks and
  shores, snowy beaches, the pale garden and the caves have none (B 8.9, last row).
- **How many**: one biome's list serves the whole chunk, read at the top of its corner
  (B 8.0.4). A chunk tries a group while a draw is below its probability, 0.1 almost
  everywhere, 0.07 in snowy plains and ice spikes, 0.04 in wooded badlands, 0.03 in badlands
  (B 8.3), so a chunk of plains has a group in about one chunk in nine (0.1/0.9 groups a
  chunk), each of the entry's count. A view of 21 by 21 chunks of plains holds some 150 to
  180 animals before failures of placement.
- **Where**: a group around one column drawn in the chunk, each member tried four times at
  columns that wander by up to four blocks (B 8.4), on the top of the column by the type's
  height map (leaves count for parrots and ocelots, B 8.0.10), on a block its rule allows
  (grass for most, sand for turtles and camels, mycelium for mooshrooms, B 8.6), lit
  above 8, and above 11 off grass, mycelium and sand (B 8.0.11). An animal stands on the
  corner of its column, not its centre (B 8.0.7), and a group's members may stand inside one
  another (B 8.0.9).
- **What they are**: colours, variants, babies (one in twenty after the first of a group;
  every rabbit after the first and every polar bear; pandas and donkeys one in five), the
  horses' and llamas' health, speed, jump and strength, pandas' genes, goats' screams and
  horns, foxes' held items, each member's yaw and hand: all drawn as the game draws them
  (B 8.7, B 10.8).

**Exact.** Both randoms that decide all of it are functions of the seed and the chunk (B 8.2,
B 8.0.2), so a player who opens the same seed in the game finds the same animals in the same
places, of the same colours, before any of them has moved. P and ADR-0028 section 4 said
colours and variants come from the entity's own random and are judged by bounds; B 8.0.2
finds that none does at `CHUNK_GENERATION`, and they are judged exactly. The one place
Clustine's input can differ is the light at a chunk's edge under an overhang (B 8.6, "Whose
light, and when"; section 3.6).

### 1.2 The structures' entities

- **Villages** (all five kinds): the villagers of the town centres and houses, unemployed,
  nitwits and babies (B 8.0.20: professions come from the brain, N10), standing, falling,
  floating and resting as N7k's reduced villager does; the town centre's iron golem
  (strolling "anywhere" without points of interest, offering poppies, B 8.0.21), its cats of
  every variant (eleven at the full moon generation sees, B 8.12), the pens' cows, pigs and
  sheep, the stables' horses, the desert's camel; the taiga armourer's two armour stands with
  their iron helmet and chestplate. **Zombie villages**: their zombie villagers (bodies until
  N9) and cats; a rare baby zombie villager on a chicken (section 3.5).
- **Pillager outposts**: the caged iron golem and the two caged allays.
- **Igloos**: the cleric villager and cleric zombie villager of the basement, exactly as
  saved in their template, drawing nothing (B 8.0.15).
- **Swamp huts**: the witch and a black cat. **Ocean monuments**: three elder guardians.
  **Woodland mansions**: evokers, vindicators with their axes, and groups of allays.
  **Ocean ruins**: their drowned. All of them bodies that float, stroll and look until N9
  (P decision 1).
- **Mineshafts**: their chest minecarts on the rails generation places today, empty (the
  structures' loot is a named difference already, section 1.3).
- **Abandoned camps** (26.3): their cushions, 224 of them over 117 templates (B 8.11).

Every template entity lands where the game puts it, turned and mirrored with its piece
(B 8.10), and every finalised one draws from the run's region random where the game draws, so
that the blocks a run places after a village piece are the game's too (B 8.17).

### 1.3 Left out, and named

- **Natural spawning**: no animal comes back to a chunk once its own have gone; water stays
  empty of fish, squid and dolphins, caves of bats, glow squid and axolotls (P 11.6: until
  N9, or N7m if it is taken). Monsters do not appear at night (N9).
- **What the structures' mobs do** waits for their steps: villagers do no work, sleep in no
  bed and trade nothing (N10); zombie villagers, witches, illagers, drowned and elder
  guardians neither attack nor burn (N9); the igloo cleric's offers are kept, not traded
  (section 8, question 7). A baby zombie villager's chicken is made, and whether it is ridden
  as a jockey waits for mobs riding mobs (named in N7e).
- **Generation's fixed values** (B 8.0.18): the difficulty is the game's default, easy, so
  the special multiplier is 0 (section 3.6); no pumpkins for zombie villagers made on 31
  October; a camel's last pose change is set at its first take, not at generation.
- **A chunk made again** under a newer generator keeps the entities it has and gets no
  template entities (P 8.6.1).
- **The light** of a mob made at a chunk's edge under an overhang can be other than the
  game's (section 3.6).
- **Order**: generated entities tick, and are found in a box, in the order of their keys,
  which are scattered (P 11.6 names the box's order already).
- **Worlds made before N7l are refused** (section 4). The flat world is made without
  animals in the game too (`FlatLevelSource.spawnOriginalMobs` is empty, B 8.0.12).
- The Nether's striders, bastions' piglins and hoglins and the End's shulkers wait for those
  dimensions.

## 2. Where the mobs are made, and how they reach the store

### 2.1 In the pipeline

Generation stays a function (ADR-0021): every entity of a chunk is made on the pool, from
the seed and the chunk's inputs, before any region holds the chunk; a region only takes in
what the store hands it, as it takes a stored record (B 11.8).

- **The structures' entities are made in the feature runs**, as the game makes them in its
  `features` step (B 8.0.14, 8.10, 8.13): a jigsaw piece's template entities at the end of
  its `placeInWorld`, after its blocks; a code piece's at the point its code makes them. Each
  goes into the run's changes of the chunk of its **position** (`WorldGenRegion.addFreshEntity`,
  B 8.2's table), which may be a neighbour of the centre: the desert's `camel_spawn` has
  `pos` (0, 0, 0) and `blockPos` (0, 0, 1) (B 8.0.14). So `Changes` (`worldgen/src/scheduler/
  output.rs`) gains `entities`, kept per chunk of the three by three like blocks, refused
  and counted by the recorder beyond it, and undone with an attempt that panics like every
  other change (`view.rs`'s `undo`). Definition D (`output.rs`'s `finish`) puts each chunk's
  entities in from the nine runs in ascending rank, as it puts in blocks.
- **The animals are made in the `Lit` task** of the centre (`pool.rs` 324–343), after its
  light, as the game makes them in `spawn` after `light` (B 8.0.1, B 8.1). It reads only the
  centre's final blocks and height maps, its light and the biomes of the nine (the zoom at
  the corner looks at the noise biomes beside it), which are the lit task's inputs already
  (B 8.16), so no task is added. `Stages` gains a fifth stage, `spawn(lit centre, the nine
  finished) -> Vec<Entity>`, nothing by default, so the scheduler's toy stages and the
  preview of bare land make no animals; `FeatureStages` gives the game's. Its entities are
  appended after the runs' (the proto chunk's list is appended in the order entities are
  added and `features` precedes `spawn`, B 8.16).
- **What makes a mob is the simulation's own code**, called as pure functions: the record
  (`EntityData::Mob`'s body, mind and type state) is the sim's encoding, and each type's
  `construct`, `finalize_spawn` and `after_finalize_spawn` hooks (`clustine-sim/src/mobs/
  types.rs` 95–130) are what an egg calls today (`region/spawn_eggs.rs` 180–290). A new
  public module, `clustine_sim::generation`, gives: making a mob for a reason
  (`CHUNK_GENERATION` or `STRUCTURE`) with a group, from a key, a position, a yaw and the
  level's draws; making an entity from a template's tag; and the spawning of a chunk, against
  a `SpawnView` (blocks, the two motion height maps, light, the zoomed and noise biomes)
  that `services/worldgen` implements over the lit centre and the nine. It needs no `Region`,
  no clock and no I/O, which the sim's `clippy.toml` already holds it to. `services/worldgen`
  depends on `clustine-sim`; `clustine-features` does not: a run makes entities through a
  maker trait in `clustine-world::generation` that worldgen's stages hand it, so the
  features crate and its tests stay free of the simulation, and a toy maker serves them.
- **The draws reach the hooks through one small trait**, `Draws` in
  `clustine-world::generation` (`next_int`, `next_float`, `next_double`, `next_bool`, as the
  sim's `Random` names them), which the sim's stream implements and an adapter gives any
  `clustine_noise::Random`: so the same hook draws from a player's stream aside for an egg
  and from the game's Xoroshiro region random for generation, and an egg's draws do not
  change. `Spawning` (`types.rs` 503–516) gains the reason, the group data (B 8.7: a group's
  members share what the first `finalizeSpawn` returned), the structure whose piece is
  placing it (the swamp hut's black cat, B 8.13; `StructureCheck` asks only that, named), and
  the moon's brightness in place of the day (a `WorldGenRegion` has no time layer, so the
  full moon, B 8.7's notes; an egg passes the clock's).

### 2.2 The record of a generated entity

What the game keeps of a mob made with a chunk is its saved form only (B 8.0.13, 8.8), and
so is Clustine's: the entity as `clustine-world/src/entity.rs` has it, with:

- `lived` 0, `fresh_at` 0, `hops` 0; the position, the yaw (the W draw × 360, unwrapped, as
  `snapTo` keeps it, B 8.4) and pitch 0 for the spawn step, the transformed position and
  yaw for a template (B 8.10); motion 0; on the ground false, as the game saves it; the
  head's and body's yaw the yaw (the load at `FULL` sets both, B 8.4, **Inferred** there).
- The type's state and body as its hooks leave them; health, air and attributes as an egg's
  mob has them before its `finalizeSpawn`.
- **Its own draws at its making** (the UUID, the constructor's yaw, `construct`'s timers, a
  salmon's size or a zombie's attribute draws, which the game takes from the mob's own
  clock-seeded random) come from a stream of their own, `Random::of_generated(seed, key)`,
  with a mark of its own beside `of_entity` and `of_entity_aside`
  (`region/random_ticks.rs` 60–95). Its first tick at `lived` 0 then draws its own stream
  afresh, as an egg's mob's does, with no count kept by any region.
- **Counters as of the first take** (P 7.6.3, B 8.16 item 3): every counter of a mob is a
  stamp against `lived` (P 11.2), so a record at `lived` 0 means the same whichever region
  takes it, and whenever: a baby's −24000, a villager baby's −21359, a goat's and a frog's
  cooldowns (N7j's encoding is held to this). The one counter of game time, the camel's last
  pose change (`max(0, gameTime − 53)`, B 8.7), is left unset at generation, and the type's
  new hook `first_take(mob, game_time)` sets it when the chunk is first taken (section 2.5).

### 2.3 Keys

The layout P 7.6.2 fixed: the top bit set, then a bijective mix of the dimension (2 bits),
the chunk's x and z (22 signed bits each, enough for the world border's 1,875,000 chunks),
whether a structure made it (1 bit) and an index (16 bits): 63 bits packed, put through a
mixer that is a bijection on 63 bits (xor-shifts and multiplications by odd constants modulo
2^63, its inverse kept beside it for tests), and bit 63 set. So the low 32 bits, the game's
`getId()` (`EntityId::game_id`), are scattered; nothing keys by them (checked by the lead:
every map keyed by `EntityId` keys by the whole key).

**Which chunk, which index.** P says "from the chunk" without saying which; this plan takes
**the chunk whose generation made the entity**: the run's centre for the structures' (class
1), the spawn's centre for the animals' (class 0), and the index the entity's place among
those that chunk's run or spawn made, counting from 0 in the order they are made (an undone
attempt gives its indices back). Not the chunk it lands in, because a run's entity is made
before definition D knows where it is in its chunk's final list, and its key has to exist
when it is made: its UUID and its own draws are keyed by it (section 2.2). The two are
unique the same way: a centre has one run and one spawn. A run or spawn that would make a
65,537th entity of its class makes no more and is marked "with a failure" (ADR-0021), which
no structure of the census comes near (B 8.11, B 8.16 item 5).

**Region keys are kept below.** `EntityIds::BLOCK_COUNT` becomes 2^31 (P 7.6.2), so no
region's block, and no stay, ever has bit 63; the store refuses to hand out a block at 2^31,
which at sixteen blocks a hello no world reaches.

### 2.4 Into the store

A generated chunk now has entities from the start, and the store keeps them by P's rules:

1. **The note writes both parts.** `Chunks::note` (`chunks.rs` 395–406) notes the chunk with
   `Parts::ALL` where it has entities (blocks only where it has none: an absent entities
   file is an empty list). The comment "a chunk as the generator made it has no entities"
   goes.
2. **The round's rule** (P 9.5.1, 10.2): `sync` puts no chunk's **first** blocks file in
   place until that chunk's entities file is in place and its directory synced. Whenever such
   a chunk is pending, the sync makes two rounds: the entities files of the first-timers,
   then everything else. A blocks file therefore always implies its entities, and **a blocks
   file means taken**. The first round may write an entities file over one already there,
   which is safe because a load never reads an entities file without its blocks file
   (`chunks.rs` 340–360 reads it only beside one); the comment says so. `Saved` keeps a
   `first` mark (the position has no blocks file yet), set by a note and kept by every save
   merged into it.
3. **A save merged into a pending note keeps the note's standing.** Today `Saved::new`
   (`chunks.rs` 197–227) clears `noted`, so a failed sync drops the merged save, and if the
   region's save again carries only its entities, the chunk's first blocks file would wait
   for a save that may never name its blocks while its entities file goes in. The merged
   save keeps `first` and is kept across a failed sync like a note; its blocks are written
   from the later chunk.
4. **Remakes keep the stored entities** (P 8.6.1, 9.5.2). `remakes` (`chunks.rs` 1138–1140)
   drops its `entities().is_empty()` condition; a load, a copy's base, a restore's base and
   a fold's base that remake a chunk answer the new blocks with the stored entities (an empty
   list included), and note only its blocks, leaving the entities file as it is. A remade
   chunk gets no template entities (named).
5. **The worker stops clearing the mark.** `saved_chunk` (`worker/src/lib.rs` 3270–3280)
   clears `generated()` on a chunk with entities so that the old `remakes` does not make it
   over them; with item 4 that is no longer needed, and it would now clear the mark of every
   chunk with an animal, which the edge's light reads (`edge/src/light_orders.rs` 416–430).
   The mark stays a statement about the blocks (ADR-0022). A chunk still as generated is
   saved whole, as now (`lib.rs` 2750–2760), so its first save writes its blocks file.

### 2.5 Taken once, and never made twice

The taking is the holder's first load (or a borrower's, or an arrival ticket's): the region
commits each entity `Whole` with its `hops` one higher (`region/entities.rs` 138–212, the
`Fresh` case of P 10.1), running `first_take` first for an entity whose key is generated and
whose `hops` is 0, which no taken entity has. From then the log and the region's saves keep
it, and the generator is never asked for its entities again: a later load finds the blocks
file.

| Case | What happens | Why once |
|---|---|---|
| Ordinary | made, handed, noted (entities file, then blocks file), taken in a commit, saved at checkpoints | the blocks file means taken |
| All the animals walk away, the record is deleted | the blocks file stays; the chunk loads with no entities | taken by the blocks file, not by the record |
| The store crashes before the note is durable | the chunk is made again at the restore, with the same entities under the same keys, and the log's `Whole`, `Gone` and `Set` by key are replayed into it (B 8.16 item 1, option b) | replay is by key; an animal that walked away is `Gone` from the remade chunk |
| A crash between the note's two rounds | an entities file without its blocks file is never read; the chunk is made again, and its next note writes the same file over it | the round's rule |
| The worker crashes after the take | its restore replays the commits, as for any entity | the log |
| A machine lost with its write-back cache | a synced write may be gone (CLAUDE.md); named, as ADR-0028's Consequences name it | not claimed |
| Made by a region that does not hold it | a copy's base, the spawn's search and the store's look-ahead ask the generator and take nothing: copies are never ticked or written (ADR-0025, section 3), and the holder's later take sends the copier `Whole` of the same keys at the same places | only a holder, a borrower or an arrival ticket takes |
| Two regions generating neighbours at once | each chunk's list is a function of the seed and its inputs, the scheduler's cache shared and its order never showing (`Pick::Shuffled`); a structure's entity is in exactly one chunk's list, that of its position, placed by the one run whose centre holds its template block (B 8.10, `TemplateStructurePiece.postProcess`) | one chunk, one holder |
| An arrival into a chunk never generated | the holder loads it with its arrival ticket, takes the generator's entities, then the arrival in the same tick (ADR-0028, section 5); a generated key never meets a region key (bit 63) | ADR-0028's own rule |
| A fold at a change of division | the fold's base of a chunk not stored is the generator's, with its entities; arrivals are written into it by key and saved whole, by the round's rule | the fold, then the blocks file |
| A remake (S8) | new blocks, the stored entities | item 4 of 2.4 |

### 2.6 What the edge and the copies see

Nothing new: a generated mob reaches clients as any mob does when its holder reports it
(B 9.8), and a copier sees it at `hops` 0 until the holder's take raises it. The new types'
data and events are N7k's (B 9.8's tables).

## 3. The draws

### 3.1 The two randoms (B 8.2, B 10.8)

- **W**, the spawn's: `WorldgenRandom(LegacyRandomSource)` with
  `setDecorationSeed(worldSeed, minBlockX, minBlockZ)` (WorldgenRandom.txt 80–112,
  `NoiseBasedChunkGenerator.spawnOriginalMobs` 1039–1081): the same seed the chunk's feature
  run starts from, but a fresh generator. `clustine_noise::WorldgenRandom::
  set_decoration_seed` is it (as `clustine-features/src/run.rs` 28–31 uses it); `nextFloat`
  is `next(24) × 2^-24` and `nextInt(n)` Java's.
- **R**, the region random: `getOrCreateRandomFactory("minecraft:worldgen_region_random")
  .at(minX, 0, minZ)` of the world's Xoroshiro (WorldGenRegion.txt 97–130, RandomState.txt
  28–61, 157–167), made anew by each `WorldGenRegion`. The spawn step's R is a new source
  equal to the one the same chunk's run began with, not its continuation (B 8.0.3).
  `clustine-features/src/context.rs` 73–96 makes it; that construction is made a public
  function of the features crate so that worldgen's spawn step makes it the same way.
  In a run, R is the one the run carries from placement to placement (`run.rs` 49–82).
- The spawn's rules are handed R and none draws (B 8.6); every `finalizeSpawn` draws from R;
  the mob's own random draws nothing of the spawning (B 10.8, section 2.2).

### 3.2 The spawning of a chunk (NaturalSpawner.txt 821–1121)

As B 8.4's listing, line for line, with B 10.8's draws:

1. `disable_mob_generation` (false for the overworld, `clustine-worldgen-data` has it) and
   the game rule `spawn_mobs`, always true here.
2. The list from the **zoomed** biome at (minX + 0.5, 319.5, minZ + 0.5), the probability
   from the **noise** biome there (B 8.0.4, 8.3); an empty list returns before any draw.
3. `while W.nextFloat() < probability`: the entry by `nextInt(totalWeight)` (the jungles'
   two chicken entries each count); the count (a uniform one draws, a constant does not);
   x then z by `nextInt(16)`.
4. For each member, at most four attempts: the top by the type's height map
   (`MOTION_BLOCKING` for the ocelot and the parrot, `MOTION_BLOCKING_NO_LEAVES` otherwise),
   `adjustSpawnPosition` (lowered into a block the type can walk through where the placement
   is `ON_GROUND`; the fox, the panda and the trader llama `NO_RESTRICTIONS`), the clamp by
   the type's **width** widened from `f32` (so a column at the chunk's edge is moved in by
   the width, an inner one keeps its integral corner), the box of `getSpawnAABB` against
   blocks and liquids (a region has no entities), the type's rule, the making, the yaw
   (`W.nextFloat() × 360`), the mob's own rules (`getWalkTargetValue >= 0` with the
   light's `f32` magic value, threshold 12; `checkSpawnObstruction`), `finalizeSpawn`.
5. **The quirk of B 8.0.6, kept**: an attempt that fails at the box, at the type's rule or
   at the making skips the step to a new column and tries the same column again; one that
   fails at the placement or at the mob's own rules, or succeeds, steps on by
   `nextInt(5) − nextInt(5)` for x then z (each pair drawn left to right), and draws again
   from the group's first column while the column is outside the chunk.

### 3.3 `finalizeSpawn` at `CHUNK_GENERATION`

Each creature's, in B 8.7's table and B 10.8's order, then the ageable tail (the first of a
group never a baby, B 8.7) and the Mob tail (the follow range's triangle unless the bonus is
there already, the hand). What the hooks gain is the group: `HorseGroupData` (the first
draws the variant, the rest share it), `LlamaGroupData`, `RabbitGroupData` (the variant drawn
for every member, the group's kept; chance 1), `FoxGroupData` (babies by the group's size,
no draw), `WolfPackData`, and the ageable group's chances (0.05; 0.2 for donkeys, mules,
camels and pandas; 1 for polar bears; none for parrots). The goat draws its horns before it
can be a baby (B 8.7's notes). An egg's mob passes no group and the reason `SPAWN_EGG`, so
its draws stay as they are.

### 3.4 The structures' draws

At the end of a jigsaw piece's `placeInWorld`, for each template entity in the template's
order whose transformed `blockPos` is in the centre chunk's box (StructureTemplate.txt
1204–1309, 2407–2461; B 8.10): the tag copied, `Pos` set, the UUID taken out, `block_pos`
replaced; the entity made; its yaw `rotate + (mirror − yRot)` in floats; `snapTo`; then, where
the settings say `finalize` and it is a `Mob`, `finalizeSpawn(…, STRUCTURE, null)` with R:
B 8.12's table, and for each template mob B 8.17's draws. The template's values that count
are kept (B 8.0.16): a follow range with its bonus skips the triangle, and a villager's or
zombie villager's `VillagerData` marks it finalised. The igloo's settings do not finalise
(B 8.0.15), the mansion's ignore entities (B 8.10).

The code pieces make what they draw for today: the swamp hut's witch and cat
(`swamp_hut.rs` 95–110), the monument's elders (`ocean_monument.rs` 155–160, `rooms.rs`
630, 655, 694), the mansion's evokers, vindicators and allays (`woodland_mansion.rs`
160–240), the ocean ruins' drowned (`ocean_ruin.rs` 179–250), the mineshafts' chest carts
(`mineshaft.rs` 288–310). Their draws stay where they are; the making joins them.

**What a run misses today, mended** (B 8.17): `jigsaw.rs`'s `single` hands the template to
`place_in_world`, which makes no entity and draws nothing for one (`clustine-structures/src/
place.rs` 9–12, 303). So the villages' and outposts' finalised mobs' draws are missing from
R after each such piece, which shows in blocks only where a later placement of the same run
reads R and places by it: the pale moss carpet of `pale_moss_patch`, only in a pale garden;
the desert pyramid's cellar comes before the villages in the step's order (B 8.17). No
judged area holds both, which is why the judge passes. N7l makes the draws, and a test of
R's state after a village piece (as the swamp hut's, `swamp_hut.rs` 161–190) shows the gap
closed before the comparisons do.

### 3.5 A zombie villager's chicken

A baby zombie villager draws for a chicken to ride; a region finds none (B 8.0.9), and in one
case of twenty a chicken is made, finalised (its own draws in the middle of the zombie's) and
ridden (B 8.12, B 10.8). The game saves the pair as the chicken carrying the zombie, the
passenger not kept by itself (`ProtoChunk.addEntity`, B 8.8). Clustine makes the unit: two
records in the chicken's chunk, each with its own generated key, the chicken's passenger the
zombie and the zombie's vehicle the chicken (ADR-0028, section 5).

### 3.6 Fixed where the game reads the clock, and the light

- **Difficulty** (B 8.0.18, 8.12): the game's is the server's at the overworld's clock and
  the moon's brightness. Clustine has no difficulty yet and every player is creative;
  generation takes the game's default, easy, for which the special multiplier is 0 at any
  time and moon. The draws of `0 × chance` are still made. The comparisons run the official
  server at easy.
- **Halloween**: never (a named difference).
- **The camel's pose**: at the first take (section 2.2).
- **The light** (B 8.6): the game spawns after the chunk's `light`, when a run two chunks
  away can still change a neighbour and the light at the chunk's edge with it; Clustine
  lights the centre from its nine finished chunks. They differ only for a mob made at the
  chunk's edge under an overhang or a tree; on open ground under the sky both are 15. Named,
  and section 6's second comparison counts it.
- Light is read from the lit chunk with the game's meaning of a section left out (sky 15
  above everything, block 0): a lookup `clustine-world/src/light.rs` lacks today (B 8.16).

### 3.7 What is not the game's

The UUID and the mob's own draws come from Clustine's streams (section 2.2), as everything a
mob draws later does (ADR-0028, section 4); the game's are clock-seeded and so in no world
the same twice.

## 4. The format, the version and existing worlds

- `FEATURES_VERSION` (`worldgen/src/features.rs` 25–31) goes to 3, and the identity gains
  `;entities {ENTITIES_GENERATION}`, a constant of `clustine_sim::generation` raised whenever
  the set of types made with chunks or by structures changes, so that a world made while
  fewer were built is refused rather than continued with a seam (as the features' identity
  does, `features.rs` 75–110). The store's `FORMAT_VERSION` (`clustine-format/src/lib.rs` 49)
  goes to 13, as P 8.6.2 and 11.2 agreed for each half.
- **What it means**: a seeded world made before N7l is refused (its identity differs), and
  so is a flat world (its format differs), which is to be made again; the owner agreed that
  worlds are refused at each half (P 11.7, question 7). A world could not be continued
  anyway: its stored chunks would never get their animals (a blocks file means taken) while
  new ones would.
- **ADR-0021, section 8's "what raises it" gains an eighth source**: `clustine_sim::
  generation` and every type's `construct`, `finalize_spawn`, `after_finalize_spawn` and
  template hook, since a change to any of them changes a generated chunk. So N9's and N10's
  edits to a villager's or a zombie's `finalizeSpawn` raise the version. The guard is test 9
  of ADR-0021 extended: its few chunks' hashes cover their entities, and a plains chunk with
  a group and a village's centre are among them.
- The raise is the step's last commit, once every type of B 8.9 and B 8.11 is built
  (section 5.0): before it, a seeded world would be refused again at the next type.

## 5. The division of work

### 5.0 When

N7l makes every creature of B 8.9 and every mob of B 8.11 and 8.13, so its end waits for
N7i's wild animals, N7j's brains (goats, frogs, camels, armadillos and allays keep their
memories in them) and N7k's bodies. Its first part (5.1, the store and the keys) depends on
none of them and can be built now, beside N7i; the delegated parts start once the types they
make are on `main`, or build against those that are and leave a test per missing type
ignored until it comes. The raise (5.6) is last.

### 5.1 The lead's part, fixed first: keys, the store, the taking (L1)

Files: `clustine-world/src/{position.rs, generation.rs, entity.rs, light.rs}` (the light
lookup's signature), `services/worldstore/src/chunks.rs`, `services/worker/src/lib.rs`,
`services/worldgen/src/{stages.rs, scheduler/output.rs, scheduler/view.rs,
scheduler/pool.rs}`, `clustine-sim/src/generation/mod.rs` (the API with stub bodies),
`clustine-sim/src/mobs/types.rs` (the hooks' new signatures: `Spawning`'s reason, group,
structure and moon; `first_take`; the template hook, all with the game's defaults),
`clustine-sim/src/region/{entities.rs, random_ticks.rs}`.

- The key layout and its mixer; `EntityIds::BLOCK_COUNT` at 2^31 and the store's refusal;
  the audit that nothing keys by `game_id`.
- `Draws`, the maker trait, `Neighbourhood`'s `entity_key` and `add_entity` with their
  undo and their recorder, `Changes.entities`, definition D's entities, `Stages::spawn` and
  the `Lit` task's call, the cache's reckoning of entities' bytes (`output.rs`'s `bytes`).
- The store's note, round's rule, `first` mark, merged saves and remakes (2.4); the worker's
  mark (2.4.5); `first_take` at the take-in; `Random::of_generated`.
- The data (L0, the lead's since it runs `cargo datagen` with the jar): the biomes'
  `spawners` of every category with their weights and counts and `spawn_costs` (N9 reads
  them too) and the probability attribute, into `clustine-data`; `SpawnPlacements`' table
  for every type (placement, height map, rule) written from B 8.5 and checked by
  `Extract.java` against the static block's map; per state and per land creature
  `isValidSpawn`, per state `isSignalSource` and `isRedstoneConductor`; the types' `immuneTo`
  tags, `summon` flags and fire immunity; the spawning's block and biome tags. `isPathfindable`
  and collision shapes are extracted already (`clustine-data` `state_path_types`).
- Tests (mine, before any part is delegated): keys distinct over every chunk within ±2^21
  sampled and every index, the mixer inverted, bit 63 always set and never in a region
  block, `game_id` spread (no two of a chunk's first 64 equal); a run's entity undone with
  its attempt and its index given back; definition D's entities in rank order and the
  shuffled pick still never showing (`scheduler/tests.rs`); on the file store with a disk
  that fails between the two rounds (`disk.rs`'s simulated disk that fails and crashes), a chunk with entities never
  has its blocks file without its entities file; a merged save across a failed sync; a
  remake that keeps an empty list and one that keeps walked-off animals; and with a **test
  generator** that makes entities with generated keys (in `worldstore/src/generation_tests.rs`):
  a chunk taken, an animal walked into the next chunk, the store killed before the note's
  sync, the region restored: the animal exists once, where it walked; the same with the
  worker killed; a copy's base of a chunk nobody holds, then the holder's take, seen by the
  copier once; an arrival into a chunk never generated, both taken.

Verified as every commit: the four commands of CLAUDE.md in a clean checkout, the worlds in
`/dev/shm`. One commit for the keys and the scheduler's contract, one for the store, pushed
before any part is delegated (a worktree starts from `origin/main`).

### 5.2 D1, the creatures' `finalizeSpawn` by reason and group (delegated)

Files: `clustine-sim/src/mobs/types/*.rs` and the types' modules N7i to N7k bring, nothing
else. Each creature's hook draws as B 8.7 and B 10.8 say for `CHUNK_GENERATION` and as B
8.12 for `STRUCTURE`, with the group of 3.3; each type's template hook reads what the census
says counts (B 8.11's last column) from the tag: a sheep's `Color`, a horse's `Variant`
(variant and markings packed), a cat's `variant` (with the old `CatType` key) and
`CollarColor`, a villager's `VillagerData` and `Age`, an iron golem's `PlayerCreated`,
`PersistenceRequired` and the attributes' modifiers for every mob, an allay's empty brain.

Tests: for every creature, the draws in order against a counting `Draws` that records each
call and its bound (B 10.8's table is the oracle), for the first of a group and a later
member; a horse herd sharing its variant; rabbits all babies after the first; a fox's babies
from the third; a goat that becomes a baby having lost a horn; the egg's draws unchanged (the
existing spawn-egg tests pass untouched); each template mob keeps what the census says and
loses the rest (a cat's variant drawn again, its collar kept; a template horse's health drawn
again; the triangle skipped where the bonus is). Verified with `cargo test -p clustine-sim`,
fmt and clippy in its worktree; its `target` removed when merged.

### 5.3 D2, the spawning of a chunk (delegated)

Files: `clustine-sim/src/generation/spawning.rs` (the loop of 3.2, `getTopNonCollidingPos`,
the placements, `isValidEmptySpawnBlock`, `isBlockDangerous`, the rules of B 8.6, the magic
value of light in `f32`), `services/worldgen/src/spawn.rs` (the `SpawnView` over the lit
centre and the nine: blocks, the two motion height maps, light, `Zoom` at the corner's top
and at each mob's block, the noise biome at the corner's top) and the light lookup's body in
`clustine-world/src/light.rs`.

Tests: on chunks built by hand, the W draws of a whole chunk against a hand-run of B 8.4's
listing (a group placed; a blocked column spending its four attempts on one column; a column
stepping out of the chunk and drawn again from the first); the clamp at an edge column for a
cow and a horse; a parrot on leaves, a fox in snow with no ground test, a cow not lowered
above a slab and lowered into two layers of snow; light 8 refused, 9 taken on grass, 11
refused and 12 taken on sand for a rabbit; a polar bear on ice only in a frozen ocean; a
turtle only below sea level + 4; the list of one biome and the probability of another at a
biome's edge; the flat world and the End making none. Verified as D1, with `-p
clustine-worldgen` beside.

### 5.4 D3, the structures' entities (delegated)

Files: `clustine-structures/src/place.rs` (`placeEntities` and the `Vec3` transform of B 8.10,
in place of "entities are not made"), `clustine-features/src/structures/{jigsaw.rs,
igloo.rs, swamp_hut.rs, ocean_monument.rs, woodland_mansion.rs, ocean_ruin.rs,
mineshaft.rs}`, `clustine-features/src/context.rs` (the maker reached from `Level`), and
`clustine-sim/src/generation/template.rs` (a template's tag read with the sim's own NBT
reader, the type's hook called; armour stands, cushions and chest minecarts made as their
records are).

Tests: every template of the census with entities placed under each rotation and mirror,
each entity's position and yaw against B 8.10's formulas worked by hand for one case of
each; `camel_spawn` turned into the neighbour chunk, made by the run whose centre holds its
block and kept in the neighbour's changes; an entity whose block is outside the centre's box
not made; R's next long after a plains village's house with a villager and after an
outpost's cage, against a fresh R taken through B 8.17's draws; the igloo's two drawing
nothing and keeping their tags; the mansion's group of allays drawing `nextInt(3)` first; the
swamp hut's witch and black cat made once, in the chunk of their block; the chicken jockey's
unit. Verified as D1, with `-p clustine-structures -p clustine-features`; against D1's hooks
once they are on `main` (`git merge --ff-only main` first).

### 5.5 The parts not delegated

The store's and the taking's rules (5.1), the wiring (worldgen's stages handing the maker to
the runs and calling D2's spawning in `Lit`), the raise (section 4), the ADRs' amendments
(ADR-0021: the entities of runs and of `Lit`, the eighth source of the version; ADR-0022: a
generated chunk carries entities; ADR-0028: generated keys, blocks below 2^31, a blocks file
means taken), the comparisons (section 6, which start the official server), the measurement
(section 7) and the roadmap's "what to try". The lead reads and tests what each part brings
before it is merged.

### 5.6 Tests from the specification (delegated, to someone who wrote none of it)

From B section 8, 10.8 and 11.8, P 7.6, 8.6, 9.5, 10.2 and this plan, without the code:
crate tests of the spawning, the structures' entities and the keys, and kill tests in the
store's manner (`worldstore/src/kill_entities.rs`) over a world whose test generator makes
entities: every process killed at every step from a chunk's generation through its note's
two rounds, its take and its animals walking off, each time every generated key existing
once; and an end-to-end test in `bin/clustine/tests/` with a seeded world: a bot's first view
shows animals in plains and none in an ocean, the same keys after the store's restart, and
across a pinned line both regions' animals once. Twice before such tests found an ordering
bug the author's own passed (CLAUDE.md).

## 6. Comparisons with the official server

All `#[ignore]`, named `official_server`, run by the lead on a quiet machine with the jar
`cargo datagen` downloads, as the terrain's are (`services/worldgen/tests/game_features.rs`).

1. **The judge, in the game's own classes** (`services/worldgen/tests/game_spawning.rs`,
   beside `game_features.rs`, its Java program extending `tests/java/GameFeatures.java`): the
   official server's classes run each chunk's features in Clustine's order, light the area,
   then call `spawnOriginalMobs(new WorldGenRegion(…))` for each chunk, and write every proto
   chunk's entity tags (`ProtoChunk.getEntities`, the saved form) and R's next long after
   each run. Clustine's scheduler makes the same area. Compared exactly, entity by entity in
   each chunk: the type, `Pos` and `Rotation` by their bits, `Age`, `LeftHanded`,
   `PersistenceRequired`, `Health`, the attributes' bases and the follow range's modifier by
   their bits, and each type's own saved fields (colour, variants, markings, strength,
   genes, horns, screaming, held items, `VillagerData`, collar, brain memories); not the UUID.
   R's long after each run compared too, which judges the structures' draws apart from the
   blocks. The areas: the 204 of the features' judge, one around the first chunk of each
   biome of B 8.9 and of each structure of B 8.11 and 8.13, two seeds, and one area found by
   a search for a village piece within reach of a pale garden's moss patch (B 8.17).
2. **A world the official server made**: the server at easy with a seed, `tick freeze` at
   once (so that nothing moves), a forceloaded area far from the spawn, saved and stopped;
   its `entities/` region files read and compared with Clustine's chunks of the same area as
   in 1, where the two have the same blocks. Differences explained by the light at a chunk's
   edge (section 3.6) or by the game's order of runs (which the judge imposes and a running
   server does not) are listed by cause; any other fails.
3. **What a client is shown** (`bin/clustine/tests/generated_oracle.rs`, with the bots as
   `mobs_oracle.rs` does): a bot at the same far place of the same seed on both servers,
   the official one frozen; the generated mobs matched by type and the nearest position
   within half a block (Clustine's may have begun to move), and their spawn packets'
   entity data, equipment and attributes compared exactly. This is what checks the new
   types' data on the wire, which the bots' shared codec cannot (CLAUDE.md).

## 7. Measurements

A flight over new land with animals (`bin/clustine/tests/herds.rs` beside N7f's cases), at a
creative player's flying speed for two minutes over plains and forest and through a village,
seed 3 and the owner's `--pin 4` line and regions that follow players; in memory
(`TMPDIR=/dev/shm`) and on the real disk with its cache, and with syncs made 22 ms slower to
stand in for the disk without it, as E7 did. Each figure says which it is from:

- the pool's time per lit chunk with and without the spawning, and the runs' with the
  structures' entities (the scheduler's `Figures`);
- animals and structures' mobs taken a second; the take-in's commit bytes and the log's
  growth a minute; the copies' updates across the line;
- chunk files a second, entities files a second, and how many syncs take two rounds and what
  the second costs (P 11.5 gives N7n the merges and splits; the flight is N7l's);
- the region's tick, its worst and its 99th centile, beside B8's flight (21 commits and 52
  chunk files a second) and N7f's herds;
- the time from a chunk's arrival at the edge to its animals being shown;
- a store killed during the flight and restarted: every generated key once (a count, not a
  timing).

And a region standing in a village (forty mobs and more), the commits a second it makes,
beside N7f's pen.

## 8. Open questions, each with my recommendation

1. **When.** N7l's end waits for N7i's wild animals, N7j and N7k; its store and keys can be
   built now. *Recommended*: build 5.1 now, the rest after N7k, the raise last.
2. **The chunk in a key.** P says "from the chunk"; this plan takes the chunk whose run or
   spawn made the entity, not the one it lands in, so that the key exists when the entity is
   made (2.3). *Recommended* as written; the reviewer to check uniqueness.
3. **The spawning inside the `Lit` task** rather than a task of its own: no new cache
   entries, and its inputs are the lit task's. *Recommended* as written.
4. **Worldgen depending on the simulation**, the features crate through a maker trait only.
   *Recommended* as written: one `finalizeSpawn` for eggs and generation, not two.
5. **Difficulty fixed at easy for generation**, special multiplier 0, until Clustine has a
   difficulty; then it would be a setting the identity names. *Recommended*.
6. **Flat worlds refused too** by the format's raise, though N7l changes nothing in them.
   *Recommended*: raise as agreed (P 11.7), one rule for every world at each half.
7. **The igloo cleric's offers and the template villagers' other saved fields** (gossips,
   `Xp`): kept as bytes of the villager's state for N10, or dropped. *Recommended*: kept,
   where N7k's villager record has a place for them; dropped and named otherwise.
8. **The mineshafts' chest minecarts** made empty now, their loot with the structures' chests
   later. *Recommended*: made, so that a mineshaft's rails are not bare.
9. **The structures' monsters** (witch, drowned, elder guardians, illagers, zombie villagers)
   placed now as bodies that stroll and do not attack. *Recommended*, as P decision 1 has it:
   a player exploring a swamp hut or a monument expects them.
10. **The light at a chunk's edge** (3.6): accepted and named rather than lighting as the
    game does at `spawn`, which would need a task with other inputs. *Recommended*; the
    second comparison counts how often it shows.
11. **The two rounds' cost** on new land without the cache (two pairs of syncs at a
    checkpoint with first-timers): accepted for now and measured (section 7); if it shows,
    the next step would write a first-timer's entities into its blocks file. *Recommended*.
12. **What to try** (written into the roadmap's first section when N7l is done):
    `cargo run --release -p clustine -- --seed 3`, fly over plains, forest, savanna, a
    desert and snow and see their animals, then into a village (villagers, cats, the golem,
    the pens and stables); open the same seed in the game with `/tick freeze` and find the
    same animals at the same blocks, of the same colours.
