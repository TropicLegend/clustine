# ADR-0021: Generation is a function, decorated in a fixed order

- Status: **Accepted**, after an independent review against the code, whose eleven
  findings are worked in (see "Review"). One of the two remaining records of step G4
  of [the terrain plan](../groundwork/terrain-plan.md), which the owner agreed on
  2026-10-10 (the roadmap, "What the owner answered on 2026-10-10"). It decides the
  shape of generation, the order of feature runs, the scheduler in
  `services/worldgen`, what the world store promises of a generated chunk, how
  generation leaves the store's thread for chunks, and what a failure in generation
  does. It changes `crates/clustine-world` (`ChunkGenerator`, a new module for the
  view of a run), `services/worldgen`, `services/worldstore` (`chunks.rs`,
  `local.rs`, `lib.rs`), `crates/clustine-rpc` (two answers of the store),
  `services/worker` (the runner: the order of loads, the hold of a link, the set of
  unreadable chunks), a world's `meta`, `docs/world-format.md`, one sentence of
  [ADR-0019](0019-data-made-from-mojangs-jar.md), and the text of test T10 of
  [ADR-0017](0017-the-end-of-the-stripes.md). What a chunk carries and how it is
  kept is [ADR-0022](0022-what-a-chunk-carries.md)'s.
- Date: 2026-10-10

Where a figure or a statement is a **guess** it says so. Statements about the code
are of the tree at `b67a1d0`. `S263:` is SteelMC's 26.3 branch at `885c4b3`; "the
audit" is the G3 audit of what a feature run reads and writes, whose findings are in
the plan's "What the first trials found"; "the plan" is the terrain plan.

Nothing was built or run for this record. The counts of sections 2 and 5 were
computed for it and again by the review (Python, a search over the lattice,
seconds); where the two differed the record has the review's.

## Context

### What the code does today

| What | Where |
|---|---|
| A generator is one function, `generate(position) -> Chunk`, and a string of settings | `crates/clustine-world/src/chunk.rs:101-107` |
| The only generator makes the same four layers everywhere | `services/worldgen/src/lib.rs:44-56` |
| Every call of `generator()` makes a generator of its own; only the store calls it outside tests | `bin/clustine/src/lib.rs:62-66`, `198-203`; `bin/clustine/src/cluster/worldstore.rs:26` |
| The store generates on its thread for chunks, inside the answer to a load, when no chunk is stored | `services/worldstore/src/chunks.rs:379-383` |
| A stored chunk that cannot be read is answered `Unreadable` and never generated instead | `chunks.rs:384-390`; `crates/clustine-rpc/src/messages.rs:378-382` |
| A region that is brought back has the block changes of its commits put into the stored chunks, and a chunk that was never stored is generated for that, on the same thread | `chunks.rs:490-508`, `559-577` |
| The same when a world is made over for another division, and when a world from before regions had a state is carried over | `chunks.rs:522-527`; `services/worldstore/src/lanes.rs:1638-1639`; `services/worldstore/src/local.rs:115` |
| There is one thread for chunks for the whole store, and it does what it is given in the order it is given | `chunks.rs:366-370`; the module's head, `chunks.rs:3-6` |
| Only the holder of a chunk has a load or a save of it passed on | `lanes.rs:680-689` |
| A load is passed to that thread by the commit thread, at once unless jobs of the same handle are held back for a group of commits | `lanes.rs:692-700` |
| A return is answered to the commit thread when its job is done, and only then is the chunk free | `chunks.rs:468-489`; `lanes.rs:805-812` |
| A barrier passes through the thread for chunks and back, and makes nothing durable | `lanes.rs:541-550`; `chunks.rs:528-533` |
| A saved chunk touches no file until the next sync; the thread syncs at a checkpoint, a return, a flush, a restore, and when 128 are pending. A sync that fails drops everything pending and loses the handles that saved | `chunks.rs:220-233`, `236-239`, `347`, `404-411`, `425`, `463`, `478`, `508`, `540-550` |
| A worker asks for a tick's loads in ascending order of position | `services/worker/src/lib.rs:1920-1929`; `crates/clustine-sim/src/region.rs:1090-1096`, `1633` |
| A worker saves a chunk only if it has unsaved changes | `services/worker/src/lib.rs:2204-2217` |
| A worker counts the answers to its loads **by position**, and takes only the answer to its latest request for a chunk | `services/worker/src/lib.rs:1138-1161` |
| A region that stops for a merge or a split asks for a save of each unsaved chunk, a checkpoint and a flush, and stands still until they are answered | `services/worker/src/lib.rs:2219-2229`; ADR-0018, "Context" |
| A runner that is stopped while a merge or a split is under way lets go without asking anything more of the store | `services/worker/src/lib.rs:2238-2240` |
| Every chunk of a hello holds its link, and what the edge sends next waits, until each is answered with a snapshot or with the word that the region does not hold it | `services/worker/src/lib.rs:2745-2761`, `2374-2403`; ADR-0012, section 4.5 |
| A chunk answered `Unreadable` is put into a set of the runner from which nothing ever takes it; a new runner starts with an empty set | `services/worker/src/lib.rs:766`, `868`, `1080-1088`, `2445`, `2759` |
| A world records `format`, `data-version` and `generator`, and is refused when any of the three differs | `local.rs:38-58`; `services/worldstore/src/lib.rs:84-89` |
| "Only changes are stored … refuses to open with others" | `docs/world-format.md:14-16` |
| When the server stops, the store is waited for until it is at rest, by the barrier | `bin/clustine/src/lib.rs:428-442` |

### What vanilla does, and why a function is not the obvious shape

The official server makes a chunk in stages, and a stage of one chunk needs earlier
stages of its neighbours: terrain needs structure starts within 8 chunks and biomes
within 1; features need terrain within 1 and write blocks within 1; light needs the
neighbours within 1 ready to be lit (`S263:steel-core/src/chunk/chunk_pyramid.rs:368-405`).
A feature run reads whatever the chunks around it hold at that moment, so the
result of the feature stage depends on the order in which chunks were decorated. The
official world is therefore not a function of seed and position: it depends on where
players walked first.

Clustine needs a function. Regions are restored, moved, merged and split; its
differential tests compare a world on one region with the same world on several; and
a store that is to have replicas later must not have generation state to replicate.
The plan weighed three shapes (its section 1) and recommends **C**: generation is
pure, and feature runs are made in one fixed order that the official server could
have taken. The owner has agreed that this is what "block for block" means where the
official server itself depends on order.

### What the trials and the audit settled

- **G2**: the official server, driven from a Java program, decorates exactly the
  chunks it is asked for in the order it is asked. So a fixed order can be judged by
  the official server itself. Nothing in this record rests on SteelMC's word alone.
- **The audit**: for every feature type and every structure piece type of 26.3, a
  run reads live blocks and live heights only within the three by three chunks
  around it, with one corner case (a geode at its far edge reads one block further
  and may schedule a fluid tick there). It writes only within the three by three.
  Data frozen before features (the two worldgen heightmaps, biomes) is not read
  beyond the three by three either.
- **The desert pyramid is not an exception.** The plan's section 1 gave it a wider
  dependency and an order within a class. Its box lies in its start chunk and the
  next, so its scan stays inside the three by three. Neither is needed.
- **Structure pieces carry state between runs** (a pyramid's settled height, a
  beached shipwreck's, a mineshaft corridor's "the spawner is placed"). Every piece
  that keeps state spans chunks at most two apart; pieces 34 blocks long or more,
  which can touch two chunks of one class, keep none.

## Decision

**A chunk is a pure function of the generator's settings and its position. That
function is: terrain for each chunk by itself; then feature runs in nine classes,
each run seeing the runs of lower classes within two chunks and nothing else; then
light from the finished three by three. The generation service computes it on a pool
with a cache of intermediate results. The store hands out what comes of it and
saves what it handed out. On its thread for chunks, a load that waits for
generation holds back nothing of another region. A placement that fails is undone
and the world goes on without it, marked; only a failure in terrain or in structure
starts leaves chunks unanswered.**

### 1. The stages, as functions

`G` is the generator's settings (section 8). All four are functions of `G` and of
what is named, and of nothing else: no clock, no thread, no earlier call.

| Stage | Function of | Gives |
|---|---|---|
| **Starts** of a source chunk `s` | `G`, `s` | the structure starts whose source is `s`, as made: for each its structure, its pieces with their boxes, and each piece's state as made |
| **Terrain** of a chunk `p` | `G`, `p`, the starts of every source chunk within 8 of `p` | the blocks after carving; the biomes; the two worldgen heightmaps; `p`'s references (which starts within 8 have a box that meets `p`) |
| **Run** of a chunk `c` | `G`, `c`, a view (section 3) | what the run changed: blocks, block entities, scheduled ticks and marks, by chunk of its three by three; the pieces whose state it changed; what its recorder counted; the placements that failed (section 5) |
| **Light** of a chunk `p` | the finished chunks of `p`'s three by three | sky and block light of `p` (ADR-0022) |

"Within *n*" is the Chebyshev distance between chunk positions throughout.

Biomes are computed inside terrain. They are a function of `G` and position and
need no stage of their own; where the official server breaks an exact tie by what a
thread found last, the plan's rule for T3 holds and is not this record's.

**"Finished" is before post-processing.** A finished chunk has the blocks the
feature stage left, with the ticks and marks that stage scheduled still waiting
(ADR-0022 carries them; the plan's phase B takes them). Until phase B a spring does
not start to flow and a marked block has not taken its shape, in what a player
sees. Entities that a feature or a piece would make are drawn from the random
numbers and not made (the plan, F2 to F7).

### 2. The order of runs, exactly

**The class** of a chunk `(x, z)` is `(x.rem_euclid(3), z.rem_euclid(3))`. **The
rank** of a class:

| | `z mod 3` = 0 | 1 | 2 |
|---|---:|---:|---:|
| `x mod 3` = 0 | 0 | 1 | 2 |
| `x mod 3` = 1 | 7 | 6 | 3 |
| `x mod 3` = 2 | 8 | 5 | 4 |

This is the plan's "even" ranking (the audit's section 7 gives it as a list). The
rank of a chunk is the rank of its class.

Two facts about the lattice that everything below uses, each checkable in a line:

- **L1.** The nine chunks of any three by three have nine different ranks.
- **L2.** Two different chunks of one rank are at least 3 apart, so the three by
  threes around them share no chunk.

**Definition D (the finished chunk).** For a chunk `e` and a number `k` from 0 to 9,
`state(e, k)` is the terrain of `e` with the changes that the runs of the chunks
within 1 of `e` whose rank is below `k` made **to `e`**, applied in ascending rank.
By L1 those ranks differ, so the order is total. `finished(e)` is `state(e, 9)`.

For a piece `P`, `piece(P, c)` is `P`'s state as made, replaced by the state that
each run `d` with `rank(d) < rank(c)` within 2 of `c` gave it, in ascending rank; if
two such runs have one rank, either order (they cannot both have changed `P`: see
"what it rests on" below). A run that sets a piece's state more than once gives it
the last.

**The run of `c`** is `decorate` on a view in which

- a block, a block entity or a final height in a chunk `e` within 1 of `c` is that of
  `state(e, rank(c))` at the start, and from then on as this run changed it;
- a piece `P` is `piece(P, c)` at the start, and from then on as this run changed it;
- everything frozen is terrain's.

So a run depends on the terrain within 2 of it and on **the runs of lower rank
within 2 of it**, and through them on theirs. `runs(c)` is the least set that holds
`c` and, with every `d` in it, every chunk within 2 of `d` of lower rank than `d`.
A run depends only on lower ranks, so there is no cycle and no chain longer than 9.

**What a finished chunk depends on.** `finished(e)` is a function of `G` and of:

- the runs `R(e)`: the union of `runs(d)` over the nine `d` within 1 of `e`;
- the terrain `T(e)`: every chunk within 2 of a chunk of `R(e)`;
- the starts: every source chunk within 8 of a chunk of `T(e)`.

A lit chunk depends on that of the nine chunks within 1 of it.

Computed for this ranking, least, mean and most over the nine classes where three
figures stand:

| | Runs | Terrain | Starts (source chunks) |
|---|---:|---:|---:|
| The run of a chunk of rank 0, 1, … 8: runs of others it depends on | 0, 2, 5, 12, 20, 33, 48, 69, 93 | | |
| One finished chunk, wherever it lies | 94 | 289 | 1,089 |
| One lit chunk | 94, 134, 157 | 289, 361, 400 | 1,089, 1,225, 1,296 |
| A chunk and its eight neighbours lit (what a joining player needs first) | 157, 182, 238 | 400, 441, 529 | 1,296, 1,369, 1,521 |
| A first view at distance 8: 329 chunks lit, 409 finished | 742, 822, 859 | 1,225, 1,337, 1,390 | 2,601, 2,777, 2,862 |
| The same with the ring of section 5 made ahead: 409 lit, 497 finished | 904, 942, 994 | 1,435, 1,489, 1,573 | 2,907, 2,993, 3,141 |
| One chunk walked along x, along z, diagonally: mean (worst step), the audit's | 28 (60), 30 (66), 51 (126) | 37 (87), 37 (87), 67 (174) | |

The runs a run depends on reach at most 5 chunks to −x, 7 to +x and 6 either way in
z; those of a finished chunk at most 8 in x and 7 in z. The view is `view_area` of
`services/edge/src/fanout.rs:2879-2895`. (The draft's counts of starts were of the
source chunks within 8 of the runs, which is another set than the one defined; the
review found it, and these are 17 % more.)

**What it rests on**, each from the audit:

1. Blocks, block entities and final heights are read live only within 1 of the run,
   and written only within 1. Then, by L2, a run cannot see or disturb a run of its
   own rank, and the runs that can have written into its three by three are exactly
   those within 2. The geode's corner case is the one known breach and is answered
   as section 3 says.
2. A piece that keeps state spans chunks at most 2 apart. Then every run that
   changed a piece which the run of `c` reads is within 2 of `c`, and two runs of one
   rank never change the same piece.
3. Data frozen before features is the same whatever has run.

The recorder (section 3) watches 1 and 2 in every fixture run. A breach of 3 has no
count of its own: it shows only as a fixture whose hash differs.

**One read of piece state that condition 2 does not cover by itself.** The official
server takes a start's reference position from the box of its first piece before it
places any piece of that start in a chunk
(`S263:steel-core/src/worldgen/feature/runner.rs:319-321`). So a run reads the
first piece's state whether or not that piece is near it. `piece(P, c)` gives it
what the runs within 2 of `c` did to that piece. Where the first piece moves (an
ocean ruin's cluster, by the audit) and a piece of the same start is placed more
than two chunks from it, the two could differ. The view counts such a read (a far
piece read, section 3); S2 reads the placers that use the reference position, and
the fixtures say how often it happens. No case is known.

**The plain model, which a test compares with.** Take a rectangle `A` of chunks.
Make the terrain of every chunk within 2 of `A`. Then for rank 0, 1, … 8 in turn, run
`decorate` for every chunk of `A` of that rank, in any order, each on **one shared
world** in which a live read within the three by three sees whatever is there at
that moment, a frozen read sees terrain, and **a live read beyond the three by
three sees terrain**. But for that last clause it is what the official server does
when it is asked for the chunks in this order; the official server answers such a
read from whatever the chunk two away holds then, which a run of the same rank
three chunks off may have written. Clustine and the model answer it from terrain
by decision, and the geode is the one case known. **For every chunk `e` whose
`R(e)` lies in `A`, `finished(e)` by definition D equals the model's chunk**: its
blocks, block entities, ticks and marks, and the state of every piece with a box in
`e`. Test 1 of section 9 checks that with toy features that keep to conditions 1 to
3, in two different orders within each rank.

**B as a switch.** One function decides which earlier runs a run sees:
`sees(d, c) = rank(d) < rank(c) && within(d, c, 2)`. The setting `order=alone`
replaces it by `false`: every run sees terrain alone, and `finished` still applies
the nine runs' changes in ascending rank. That is the plan's shape B. It is known to
be wrong where two chunks' features meet; it exists so that T8 can measure what C
costs and so that the difference can be counted. It is part of the generator's
identity (section 8): a world is one or the other for good. Only `order=nine` is
ever claimed to be block for block.

### 3. The view of a run: `Neighbourhood`

In `crates/clustine-world`, a module `generation`, fixed in G5. The ported features
and structure pieces are written against it and against nothing else of the world.

```rust
/// What one feature run sees and changes: the three by three chunks around
/// `centre()` as the runs before it left them, and what was fixed before features
/// around that.
pub trait Neighbourhood {
    fn centre(&self) -> ChunkPos;

    // Live: the three by three.
    fn block(&mut self, at: BlockPos) -> BlockState;
    /// False if the write was refused: outside the three by three or the height.
    fn set_block(&mut self, at: BlockPos, state: BlockState) -> bool;
    fn block_entity(&mut self, at: BlockPos) -> Option<&BlockEntity>;
    fn set_block_entity(&mut self, at: BlockPos, entity: Option<BlockEntity>) -> bool;
    /// One of the four final heightmaps, as the blocks are now.
    fn height(&mut self, map: FinalHeightmap, x: i32, z: i32) -> i32;

    // Frozen: terrain's, five by five.
    fn worldgen_height(&mut self, map: WorldgenHeightmap, x: i32, z: i32) -> i32;
    fn biome(&mut self, at: BlockPos) -> Biome;

    // What generation leaves for later (ADR-0022, phase B).
    fn schedule(&mut self, tick: ScheduledTick);
    fn mark(&mut self, at: BlockPos);

    // Structures.
    /// The starts that the centre's references name, as made.
    fn starts(&self) -> &[StartRef];
    fn piece(&mut self, piece: PieceKey) -> PieceState;
    fn set_piece(&mut self, piece: PieceKey, state: PieceState);

    /// Places one thing: a placed feature, or one piece of a start. Everything
    /// `place` does to the view is kept if it returns, and undone if it panics.
    /// Returns whether it was kept.
    fn attempt(
        &mut self,
        what: Placing,
        place: &mut dyn FnMut(&mut dyn Neighbourhood),
    ) -> bool;
}
```

`PieceKey` is the source chunk, the structure's index and the piece's index in its
start. `PieceState` is what a piece keeps **between runs**: a shift of its box in y,
whether its height is settled, and sixteen flags. (**Guess** that this holds every
kind. The audit found nothing but a height, a shift in y and a few booleans. The
port's pyramid keeps more in the same struct, a list of positions and one position
for its archaeology (`S263:steel-worldgen/src/structure/desert_pyramid.rs:18-27`),
which its own comments call per-run: every run builds them anew. G5 reads the
port's piece types, confirms for each field whether a later run reads it, keeps
what is per-run local to the run, and fixes `PieceState`.) A start's own box is
derived from its pieces and is not kept.

Reads take `&mut self` because the recorder counts them.

| Asked | Where | Answered | Recorded |
|---|---|---|---|
| A block, a block entity, a final height | in the three by three | **live** | no |
| | in the ring around it (the five by five) | from **terrain**: the block as terrain left it, the height that terrain's blocks give | yes: a far read |
| | beyond the five by five | from terrain, which the view fetches for that chunk through the scheduler (it is a function, so this is still pure) | yes: a read beyond the margin |
| | above or below the dimension | void air; the height the port's rule gives (**guess**; the port decides and the fixtures judge) | no |
| A worldgen height, a biome | in the five by five | **frozen**: terrain's, whatever has run and whatever state a neighbour is in | no |
| | beyond | from terrain, fetched | yes: a read beyond the margin |
| A write of a block or a block entity | in the three by three | done; the final heightmaps of that chunk follow it | no |
| | outside | **refused, silently**, as the official server's region refuses it (`S263:steel-core/src/worldgen/region.rs:327-332, 515-519`, by the audit) | yes: a refused write |
| A tick or a mark | in the three by three | kept, with the chunk it lies in | no |
| | outside | **dropped** | yes: a dropped tick or mark |
| A piece's state read | a piece whose box meets a chunk within 2 of the centre | `piece(P, c)`, then as this run set it | no |
| | any other piece | the same | yes: a far piece read |
| A piece's state set | a piece whose box lies in chunks at most 2 apart | kept | no |
| | any other piece | kept | yes: a far piece |
| An entity to be made (a witch, a guardian, a chest minecart) | anywhere | not part of the trait: the port draws the random numbers the game draws and makes nothing | no |

**The recorder** is a part of every run's result: six counts and the first sixteen
events, each with its kind, its position and what was being placed. It never
panics. The scheduler adds the counts up and logs a line when a run recorded
anything. What the counts must be is section 9's.

**`attempt`.** The port's runner wraps every placed feature and every piece of a
start that it places in one `attempt`. The view keeps, from the start of an attempt,
what it would take to put back everything the attempt does: each block and block
entity it overwrote, the ticks, marks and piece states it added or replaced, the
recorder's counts. If `place` panics, the view catches the panic, puts all of that
back, notes the failure (what was being placed, and the panic's message) and
returns false; the runner goes on with the next placement. Attempts do not nest.
What makes that sound:

- A placement keeps nothing outside the view but its own locals and the random
  source it was handed. The official server reseeds for every placed feature and
  every structure (`S263:…/feature/runner.rs:163-164, 302`, by the audit), so the
  port hands each placed feature a source of its own, and a feature that fails does
  not move the next one's numbers. The pieces of one start share a source; after a
  piece failed, the next pieces draw from wherever it stopped. That is still a
  function of `G` and position, which is all that is asked of a chunk that has a
  failure in it.
- A panic that is not inside an attempt (in the runner's own code between two
  placements) ends the run there. What the attempts before it did is kept, and the
  run has one failure more, of the kind "the run was cut short".
- So **a run never fails as a whole.** It gives a result, with zero or more
  failures in it.

**The geode.** A crystal or a crack at 16 blocks from a geode's origin reads its
neighbour at 17 and may schedule a fluid tick there (audit, section 5.1). With the
origin in the chunk's outermost column that is one block beyond the three by three.
The read is a far read and is answered from terrain; the tick is dropped. So in
that case Clustine's chunk can differ from the official server's by which way one
amethyst bud points and by water or lava beside a crack that does not start to flow
until something updates it. Nobody notices either. No mark is known to be dropped
(the audit has marks only in the writer's own column). The fixtures count how often
it happens (the audit's estimate: well under one geode in a thousand). The five by
five of terrain is kept for this and as a margin; it is 11 % of the terrain of a
first view.

### 4. The interfaces

The scheduler is written against a trait that the real generator and the tests' toy
generators implement. In `services/worldgen`:

```rust
pub trait Stages: Send + Sync + 'static {
    fn settings(&self) -> GeneratorSettings;
    fn dimension(&self) -> &'static DimensionType;
    /// The structure starts whose source chunk is `source`, as made.
    fn starts(&self, source: ChunkPos) -> Starts;
    /// The chunk after carving. `starts` answers for every source chunk within 8.
    fn terrain(&self, position: ChunkPos, starts: &dyn StartsNear) -> Terrain;
    /// One feature run: structures and features of `centre`, in the game's steps,
    /// each placement in an `attempt` of the view.
    fn decorate(&self, view: &mut dyn Neighbourhood, centre: ChunkPos);
}
```

`Terrain` holds the sections and biomes as ADR-0022's chunk holds them, the two
worldgen heightmaps (256 heights each), and the references. `Starts` and `StartRef`
are `clustine-structures`' types behind `clustine-world`'s `PieceKey` and
`PieceState`.

What the store sees, in `crates/clustine-world` (it replaces
`chunk.rs:101-107`):

```rust
pub struct GeneratorSettings {
    /// Everything that decides what is produced, but the version.
    pub identity: String,
    /// Raised whenever a change makes any chunk come out otherwise (section 8).
    pub version: u16,
}

/// Generation failed for this chunk. The generator has logged why.
pub struct Failed;

pub type Done = Box<dyn FnOnce(Result<Chunk, Failed>) + Send>;

/// A request that is under way. Dropping it says that nobody waits any more.
pub struct Wanted(Option<Box<dyn FnOnce() + Send>>);

impl Drop for Wanted {
    fn drop(&mut self) { /* calls what it holds, once */ }
}

pub trait ChunkGenerator: Send + Sync {
    fn settings(&self) -> GeneratorSettings;
    /// The whole chunk: finished, lit, marked as generated by `settings().version`.
    /// Panics where generation fails; for tests and for generators that cannot.
    fn generate(&self, position: ChunkPos) -> Chunk;
    /// Has `done` called once with the chunk, on any thread, perhaps before this
    /// returns. After the `Wanted` is dropped, `done` may never be called.
    fn request(&self, position: ChunkPos, done: Done) -> Wanted {
        // Provided: generate here and now, catching a panic as `Failed`; the
        // `Wanted` it returns holds nothing.
    }
}
```

`spawn()`, which the plan lists with these, is W6's and T8's and is added there.

The flat generator keeps `generate` and gets the provided `request`; it marks its
chunks as generated by its version 1. The scheduler implements `request` itself and
`generate` as a request that is waited for.

**One generator, made once, for the store alone.** Today each call of
`generator()` makes one (`bin/clustine/src/lib.rs:62-66`), which costs nothing for
four layers. A scheduler has threads and a cache: it is made once where the store
is made and handed to nothing else. Nothing but the store calls a generator outside
tests; `spawn_point` reads the flat generator's height
(`bin/clustine/src/lib.rs:68-71`) and is W6's.

### 5. The scheduler in `services/worldgen`

`Scheduler<S: Stages>` implements `ChunkGenerator`. It lives in the process that
holds the store.

**Tasks.** Five kinds, each keyed by a chunk position, each a function of its
inputs as section 1 has them:

| Task | Needs done first |
|---|---|
| `Starts(s)` | nothing |
| `Terrain(p)` | `Starts(s)` for the 289 `s` within 8 of `p` |
| `Run(c)` | `Terrain` within 2 of `c` (25); `Run(d)` for every `d` within 2 of `c` with `sees(d, c)`; the starts `c`'s references name |
| `Finished(e)` | `Terrain(e)`; `Run(d)` for the nine `d` within 1 of `e` |
| `Lit(p)` | `Finished` for the nine chunks within 1 of `p` |

`Lit(p)` is the chunk a request is answered with. It carries "as generated by
version *n*" and, where it applies, "with a failure" (below).

**The pool.** A fixed number of threads of the scheduler's own, named
`clustine-generation-<i>`: by default two thirds of the processors, at least one
(the plan's section 6 reckons with four of six), set by `--generation-threads`. A
thread takes the task that is ready and belongs to the oldest request; a task that
only the look-ahead wants comes after every task a request wants. A task runs to
its end on one thread. Nothing in a task waits for another task: a task is only
made ready when what it needs is done. (A read beyond the margin, section 3, makes
its terrain on the thread that asked, and is expected never to happen.)

In the one process the owner runs, the pool shares the processors with the region
runners, which tick twenty times a second. Nothing makes the pool give way to
them. T8 watches the regions' tick times while a first view is generated from
nothing; **a tick that takes longer than 50 ms there fails the measurement**, and
the default share is lowered until none does.

**The order of work never shows in the result.** Every task is a function of its
inputs, and inputs are complete before it starts. Test 2 runs the same requests on
one thread, on eight, and with the ready tasks taken in a scrambled order, and
compares every chunk.

**The cache** holds the results of tasks, not chunk states: a chunk passes through
nine states on its way and none of them can be dropped alone (the plan, section 1).

- What it holds: starts, terrain, **run results** (the changes of one run, by chunk,
  with its piece states, its recorder and its failures), finished chunks, and lit
  chunks that were made ahead and not yet asked for. A lit chunk that was handed
  out is not kept: the store has it.
- **Its bound is in bytes: 512 MiB by default, `--generation-cache <MiB>`.** Each
  entry reckons its own size. When the cache is over the bound, results that no
  request under way needs are dropped, the least recently used first. Results that
  a request under way needs are never dropped, so the bound can be passed while
  many requests are under way; the scheduler logs a line when that happens.
- Dropping never changes a result: what is needed again is made again and is the
  same. What it can cost is work: a run that is needed again needs its lower runs
  again, up to 93 of them.
- **Guess**, to be measured in W5 and written into this record: terrain 30 to 45 KB
  a chunk, a run result 10 to 30 KB (the plan's guess), a finished chunk as terrain.
  Then a first view holds about 90 MB, and the default bound is five such views.
- **It is empty after every start, and stored chunks cannot stand in for run
  results.** The first step beyond the rim of what is stored costs the cones from
  nothing: for a patch of five by four chunks the audit has 190 to 238 runs and 460
  to 529 terrain, against 28 runs for a step that finds its neighbours cached. That
  is the first walk outward of every session. W5 times it. The spawn area that T8
  makes "into the cache" is gone at the next start likewise unless it is stored;
  T8 decides that.

**What is computed for a first view.** A region that is granted its player's view
asks the store for 329 chunks. For them the scheduler makes, on average over where
the view lies on the lattice: 2,777 `Starts`, 1,337 `Terrain`, 822 `Run`, 409
`Finished`, 329 `Lit` (section 2's table).

**Look-ahead.** The work comes in lumps (the lattice has period 3: a step along z
costs 30 runs on average and 66 at worst). So for every chunk that is asked for,
the scheduler also wants `Lit` of its eight neighbours **ahead**: after everything
any request wants, never answered to anyone, kept in the cache. For a standing
view that is the ring around it: 80 chunks lit ahead, 120 runs and 152 terrain
more. A later request for such a chunk is answered from the cache.

**Walking.** Every chunk walked along an axis brings 19 new chunks into view. What
they need beyond what is cached is the audit's: 28 to 30 runs and 37 terrain on
average, 51 and 67 diagonally. With the ring made ahead, a step's requests are met
from the cache and the pool works on the next ring.

**Cancelling.** A request that is dropped takes its claim off every task. A task
that nothing claims any more and that has not started is forgotten; one that runs
is finished and its result is kept, because it is right and may be wanted. The
store drops a request when the handle that asked is lost or has given the chunk
back (section 7).

**Failures.** Two kinds, by whether what failed can be left out.

| What panics | What happens | What a player has |
|---|---|---|
| A placement inside a run (a feature, a piece) | the view undoes it (section 3); the run goes on and gives its result with the failure in it | the world without that one tree, vein or piece's part in that chunk |
| `Starts(s)` or `Terrain(p)`; `Finished`, `Lit` or the scheduler's own part of a run | the task is failed; every task that needs it is failed without being run; a request whose `Lit` is failed is answered `Failed` | chunks that do not come (section 7, "`Unreadable`") |

**Every failure is recorded** when it happens: one line at the level of an error
with the seed, the dimension, the chunk, what was being placed (the feature's or
the structure's name, or the kind of task), the generator's version and the
panic's message; and a count of failures that the scheduler keeps and the store's
process logs when it stops. Nothing is written into the world for it but the mark
below.

**A chunk with a failure is marked.** A lit chunk is "as generated by version *n*,
**with a failure**" if any run it depends on (the `R` of the nine chunks within 1
of it) has a failure. That is more chunks than lack anything, on purpose: a later
run may have placed otherwise because an earlier one failed, and light follows the
blocks. One failed placement marks **25 to 289 chunks**, by the rank of its run (9
to 225 finished chunks depend on one run and 25 to 289 lit ones; computed for this
record and again by the review). ADR-0022 carries the mark: one bit beside the
version, in the chunk and in its file, cleared with the version by the first
change. **A marked chunk is whole and playable**; the mark only says that a newer
generator should make it again (section 6, S8).

**What a failure in terrain or in starts costs**, since nothing of those can be
undone piece by piece (computed, for every class):

| Failed | Chunks that cannot be finished | Chunks that cannot be lit, so are answered `Unreadable` | Across |
|---|---:|---:|---|
| `Terrain(p)` | 225 to 324 | 289 to 400 | 17 to 20 chunks |
| `Starts(s)` | 1,089 | 1,225 | 35 chunks |

A failed `Starts(s)` is not taken as "no start there": the terrain around a start
is shaped for it, so that would change terrain and everything on it for 1,225
chunks without a hole to show it, and would be stored as generated. Structure
starts are where the port's `todo!` is likely; the fixtures of T7b and S2 run starts
over wide areas before a world with structures is walked, and a start that panics
there is fixed there.

**A failed task is remembered for as long as the process runs**, so that the same
panic is not met again and again: the generator and its inputs are the same, so
the panic is. It is not written anywhere. A process that is started anew, with
whatever generator, tries again.

**Dropping the scheduler** ends its threads and waits for them.

### 6. What the store promises now

The thread for chunks answers a load of a chunk that is not stored with the
generator's chunk, as today. New:

- **S1. What a region was handed is what the world holds there.** From the moment
  a holder is answered `Loaded` with a generated chunk, every later load of that
  position is answered with that chunk, until a save puts another in its place or
  S8 remakes it.
- **S2. It is noted behind the answer.** With the answer, the thread notes the
  chunk for writing as a save is noted (`Chunks::save`, tick 0; `chunks.rs:220-233`),
  unless a chunk is stored for the position by then. A note is written and made
  durable **by the next sync the thread makes**: a checkpoint, a return, a flush
  or a restore of any handle, the store's orderly stop (S7), or when **512 notes**
  are pending. Notes have that limit of their own and do not count towards the 128
  saves of `PENDING_LIMIT` (`chunks.rs:347`), so that a first view's 329 notes do
  not make the thread sync twice in the middle of a join. The answer waits for
  nothing.
- **S3. Until it is durable, purity covers it.** A crash loses the note. The chunk
  is then made again when it is next needed and is the same, because the generator
  and its settings are the same.
- **S4. It is marked.** A chunk is stored with "as generated by version *n*,
  unchanged", and with "with a failure" where section 5 says so (ADR-0022: a number
  and a bit in the chunk's file, both cleared once anything changed it).
- **S5. Nobody's handle is tied to it, and a failed sync does not lose it.** A note
  is not counted among the saves of the handle that was answered
  (`ChunkService::saved`, `chunks.rs:356-360`, `406`). If a sync fails, the handles
  that saved are lost as today (`chunks.rs:540-550`) and their saves are dropped as
  today (`chunks.rs:236-239`); **the notes are kept pending** and the next sync
  tries them again. (A handle that was only handed generated chunks is not lost by
  a failed sync and will never save those chunks itself,
  `services/worker/src/lib.rs:2204-2217`; without this the chunks it holds would be
  in no file.) Where a save and a note of one position are pending, the save is
  what is pending; it is the chunk with a change in it.
- **S6. What nobody was handed is not saved**: chunks made ahead, loads that were
  withdrawn (section 7), and loads whose handle was lost. What **was answered** is
  noted, whether or not anyone looked at it: a region can have stopped wanting a
  chunk while its load was under way, and throws the answer away
  (`services/worker/src/lib.rs:1138-1161`). So "stored once it has been shown" is,
  exactly, "stored once a region was answered with it". Section 7's withdrawal
  keeps the difference small.
- **S7. An orderly stop leaves nothing behind.** `Store::close` is new: the barrier
  of `Store::flush` (ADR-0017, section 5.5), whose job on the thread for chunks
  **syncs** what is pending before it passes on. `Server::stop` calls it where it
  calls `flush` today (`bin/clustine/src/lib.rs:428-442`), and so does the stop of
  the store's own process (`bin/clustine/src/cluster/worldstore.rs:36-41`).
  `Store::flush` itself is as it is and makes nothing durable, and nothing is
  written when a store is dropped. So a runner that was abandoned in the middle of
  a merge (`services/worker/src/lib.rs:2238-2240`) costs no note either. If the
  sync of a close fails, `close` returns the error and the notes are lost with the
  process; S3 covers them unless a newer generator follows.
- **S8. A chunk with a failure is made again by a newer generator, if nobody
  changed it.** When a load finds a stored chunk that is marked "as generated by
  version *n*, with a failure" and the generator's version is above *n*, the chunk
  counts as not stored: the generator is asked, and its chunk is answered and
  noted in the old one's place. If the newer generator fails for it outright, the
  stored chunk is answered as it is. A chunk that was changed has lost the mark
  and is never remade.

**A world accepts a newer generator.** `meta` gets a line `generator-version`
(section 8).

- It is written **when the world is made**, with the version of the generator that
  made it.
- A store with a generator of a **higher** version opens the world and changes
  nothing. It logs one line with both versions.
- **The line is raised when the first chunk of the higher version is about to be
  stored**: in the sync that would put the first file in place that holds a chunk
  the higher version made, or a chunk changed from one, the thread first replaces
  `meta` with the higher version and syncs it and its directory (as when it is
  made, `local.rs:65-66`), and only then puts the chunk files in place. So no file
  of a world ever holds a chunk of a version above the one its `meta` names.
- So **trying a newer build and going back is allowed until something is stored**:
  looking at the world's log lines, or stopping before any new chunk was handed
  out and synced.
- A store with a **lower** version than the world's stops:

```text
This world holds chunks made by generator version 7, and this server has version 5.
Start it with a server that is at least as new.
```

Stored chunks stay as they are, whichever version made them, but for S8; chunks
made from then on are the new generator's.

**The limits that follow, named:**

- **A seam.** Where a newer generator makes a chunk beside one that an older
  generator made, and the two differ there, the two do not fit: a tree cut at a
  chunk border, an ore vein that stops. A few blocks, at the rim of what had been
  seen, and around a chunk that S8 remade. The official server has the same
  between versions.
- **A crash followed at once by a newer generator.** Chunks that were handed out
  and not yet durable (at most 511 notes, for at most a checkpoint's interval, five
  minutes by default) are made again by the newer generator. Where a player
  changed blocks in one of them, those changes are in the log and are put into the
  new chunk (section 7, "Restore"). So a torch can hang where the newer generator
  grew no tree. It needs a crash, a new binary before the next start, a fix that
  touches that very chunk, and a change made in it within those minutes. It is
  accepted and named; what would close it is choice 3.

**`docs/world-format.md` then says**, in place of its first principle
(`world-format.md:14-16`):

> - **What a region was given is stored.** A chunk that is generated for a region is
>   noted behind the answer and becomes durable with the next sync the store makes,
>   at the latest when the store is stopped in order. Until then it is made again
>   when needed, and is the same. A chunk that was never handed to a region is not
>   stored. Every stored chunk says whether it is exactly as a generator made it,
>   which version of the generator, and whether something failed while it was made.
> - **A world is tied to the generator it was made with, and takes a newer version
>   of it.** `meta` records the generator's settings and the highest version that
>   made a chunk the world stores. A server with other settings, or with an older
>   version, is refused. One with a newer version opens the world: what is stored
>   stays as it is, what is made from then on is the newer generator's, and a chunk
>   that was stored with a failure and never changed is made again.

and under "What is saved when": "A chunk that was generated for a region is noted
for writing when the region is answered, and is written with the next checkpoint,
return, flush or opening of any region, when 512 such chunks wait, or when the
store is stopped."

### 7. Generation off the thread for chunks

**The rule: only the computing of a chunk that is not stored happens elsewhere. A
load that waits for it holds back the later jobs of its own handle that are not
loads of other chunks, the later jobs of anyone that name the same chunk, and what
waits for everything. It holds back nothing else: nothing of another region.**

The draft had one order for the whole thread. The review showed what waits then:
every other region's stored loads, returns and checkpoints, and the checkpoint a
merge or a split stands still for, behind whatever generation any region asked
for, without a bound. That is what ADR-0018 removed, back by another door, and it
is gone from this record.

**What the thread is given, and what each waits for.** `X` is the handle a job is
of. "Names `c`": a load or a save of `c`, or a return that gives `c` back.

| Job | Waits until these are done | Evidence that today's order needs no more |
|---|---|---|
| **Load** of `c` by `X` | every earlier job, of any handle, that names `c`; every earlier restore and fold | the worker counts answers by position (`worker/src/lib.rs:1138-1161`); a load must find an earlier save of its chunk (`messages.rs:276-285`) |
| **Save** of `c`, **checkpoint**, **flush**, **return** by `X` | every earlier job of `X`, loads among them; every earlier job of any handle that names a chunk this job names; every earlier restore and fold | a checkpoint and a flush speak of the saves before them; a return frees chunks only after the saves before it; all of one handle |
| **Restore** (a region is opened with commits to put into chunks) | every job given before it | it reads and writes chunks of a region whose earlier owner may have jobs under way |
| **Fold** | every job given before it; and it keeps the thread until it is done | it runs only while a world is made over, before any region is open (`lanes.rs:1638-1645`) |
| **Barrier** | every job given before it | it promises rest (ADR-0017, section 5.5) |

Nothing waits for a barrier. Later jobs wait for a restore only as the table says:
a later restore or fold, and a later job that names a chunk the restore changes
(the restore counts as naming every chunk its changes lie in).

**Why no two regions' jobs on one chunk are ever unordered.** Only the holder's
loads and saves are passed on (`lanes.rs:680-689`). A chunk changes its holder
only by: a return, which is the old holder's own job behind all its earlier jobs,
and whose answer to the commit thread comes when the job is done
(`chunks.rs:468-489`), before any grant; a merge or a split, before which the
regions checkpoint and flush, each behind all its handle's earlier jobs; or the
loss of a handle, after which its waiting loads are taken out and the next owner
comes by a restore, which waits for everything. The table's "any handle that names
the same chunk" is kept besides, so that this does not rest on an argument alone.

**How the thread does it.**

1. It keeps what it was given, with the order it came in, and does a job when what
   the job waits for is done. Among the jobs that can be done, the oldest first.
2. A **load** that can be done is *looked up* (`Chunks::load`):
   - its handle is lost: it is dropped, as today (`chunks.rs:376-378`);
   - a chunk is stored, and S8 does not apply: answered `Loaded` at once;
   - it cannot be read: answered `Unreadable`;
   - nothing is stored, or S8 applies: the generator is asked (`request`), the
     handle is answered **`Generating { position }`** (new, below), and the load
     *waits*.
3. A load that waits is **answered when the generator answers**: `Loaded`, and the
   note of S2; or, if the generator failed, `Unreadable { position }` and no note
   (under S8: `Loaded` with the stored chunk).
4. A load that waits and whose handle is lost is taken out, its request dropped,
   nothing answered, nothing noted.
5. **Withdrawal.** When a return by `X` that names `c` is given to the thread while
   a load of `c` by `X` waits, that load is taken out, its request dropped, and
   `X` is answered **`Withdrawn { position }`** (new) in the load's place; nothing
   is noted. The return then no longer waits for it. A region gives a chunk back
   when nothing has used it for 600 ticks (ADR-0017, section 3.2), so a load
   nobody wants is under way for thirty seconds at most.
6. A **restore** that can be done looks its chunks up, asks the generator for
   every one that is not stored, and **waits without keeping the thread**: other
   jobs are done meanwhile by the table. When all have come it applies the changes,
   saves, syncs and answers, as today (`chunks.rs:490-521`).
7. While the thread syncs, it does nothing else. That is the one thing a job of
   one region can still wait for on account of another: a sync that another
   region's checkpoint, or 512 notes, set off.

How the thread waits for a job and for a generator's answer at once is the
builder's: an inbox that both are put into, or a second channel that it reads when
nothing can be done. A fold waits for its chunks on a channel of its own, since it
keeps the thread. Whatever is chosen, the thread must end as it does today, when
the last sender is gone and no request is under way (test 7j).

**The two new answers** (`StoreReply`, `crates/clustine-rpc/src/messages.rs:373`):

- `Generating { position }`: the chunk is not stored and is being made; `Loaded` or
  `Unreadable` follows. The worker uses it for the hold of a link (below) and for
  nothing else.
- `Withdrawn { position }`: this load is not answered because the region gave the
  chunk back. The worker counts it as the answer to one load of that position
  (`answers_the_latest_load`, `worker/src/lib.rs:1149-1161`) and does nothing else
  with it. It has to be an answer: the worker's count of loads by position would
  otherwise stay one too high, and the answer to its next load of that chunk would
  be thrown away. (The review proposed withdrawal without a new message; that
  count is why there is one.)

**S1 to S8 under this order.**

| | Holds because |
|---|---|
| S1 | A later load of `c` waits for every earlier job that names `c`, so it is looked up after the note or the save. Two loads of `c` that are both generated are answered with equal chunks, by purity, and the first answer notes |
| S2 | The note is made in the step that answers. "Unless a chunk is stored by then" matters only for the second of two generated loads of one chunk |
| **The note never replaces a region's save** | A save of `c` waits for every earlier job that names `c`, so it comes after the answer and the note of every earlier load of `c`. A load of `c` that is given after a save of `c` finds the save and is not generated. A load that waits when its chunk changes holder cannot be: see "why no two regions' jobs" |
| S3, S4 | Nothing of the order |
| S5 | `Chunks::sync` keeps the entries that are notes when it fails |
| S6 | Rules 4 and 5 note nothing |
| S7 | The barrier of `close` waits for every job before it, so for every answer and its note, and then syncs |
| S8 | The look-up of rule 2 |

**A checkpoint that a merge or a split waits for never waits behind another
region's generation.** By the table it waits for its own handle's earlier jobs,
and for earlier jobs on the chunks it saves, which are its own handle's too. What
it can wait for besides is rule 7's sync, and the region's **own** loads that are
still being generated when it stops.

- **The test** (7e below): with one handle's load held in the generator, another
  handle's saves, checkpoint and flush are answered, and its state file is in what
  a crash would leave.
- **The bound.** ADR-0017, section 9.7, set what a crowd bears at a merge or a
  split: half a second in the middle and a second at worst. ADR-0018 measured,
  after it was built, 0.31 to 0.41 s for a crowd of a hundred. W5 runs that
  measurement (`bin/clustine/tests/crowds.rs`) with a generator that takes as long
  as the real one is measured to take, **while a bot of a third region joins far
  out and has its first view generated from nothing during every split and every
  merge**. The crowd's wait must stay within ADR-0017's second at worst and within
  0.2 s of what the same run gives without the third region; W5 writes both
  figures into this record. If it fails, notes get a thread of their own to be
  written on, which takes rule 7's sync off the path.
- The region's own loads: a region whose player is at the rim of what exists when
  it is merged waits for those loads. With the ring made ahead they are met from
  the cache. W5 times a merge of two regions whose players both walk outward, to
  the same second; choice 2 has what would be turned.

**What waits for a load in the worker and at the edge.**

- **A player who joins.** They are placed in the home chunk, the region claims
  their view, and the runner asks for the loads. Today those go out in ascending
  order of position (`worker/src/lib.rs:1920-1929`), so the chunk the player
  stands in would be asked for after half the view. **Changed: the runner sorts a
  tick's loads by the distance to the nearest of its region's players, nearest
  first** (ties by position), before it asks. The simulation's own order is not
  touched (`crates/clustine-sim/src/region.rs:1633` stays true). The pool works
  for the oldest request first, so the player's own chunk and its eight neighbours
  are made first: 157 to 238 runs and 400 to 529 terrain from nothing (section 2),
  and nothing where T8's spawn area is cached or stored. The client leaves its
  loading screen when the chunk it stands in has come (**guess**). **W5 and T8
  report the time to the first chunk and to the last, not only to the last; a
  first chunk later than 2 s after a join far from anything stored fails the
  measurement** on the owner's machine.
- **The others of that edge, when someone joins.** A join is no hello and holds no
  link. Their own loads are not behind the joiner's: a load waits only for jobs on
  its own chunk. Their region's saves and checkpoints are behind the joiner's
  loads if the joiner is of the same region (the table); that is the home region
  at the spawn, whose surroundings are stored after the first join.
- **The hold of a link** (ADR-0012, section 4.5). Every chunk of a hello holds the
  link until it is answered with a snapshot, and a snapshot needs the chunk loaded
  (`worker/src/lib.rs:2745-2761`). A hello comes after a merge, a split, a move, a
  restore, or a link that broke, and names every chunk its viewers see. Chunks
  that were handed out before are stored or pending and are loaded at once. But
  chunks at the rim whose first generation is still under way would hold **all
  input of that edge's players to that region** until they are made: seconds.
  **Changed: a chunk that the store says is `Generating` no longer holds a link.**
  The runner keeps the positions it was told so until their `Loaded` or
  `Unreadable` comes; such a position is let go of in every link's hold when the
  word comes, and is not put into the hold of a later hello
  (`worker/src/lib.rs:2759`, beside the test for unreadable chunks).
  - Why that keeps what the hold is for. The hold is there so that "nothing an edge
    sends again after a restore reaches a tick before the region knows, of every
    chunk its players can reach, who holds it". The region knows that when the
    claim is answered; `Generating` comes after the grant.
  - What an edge sends again acts on chunks a client had on its screen, and a
    client has only chunks a region was answered with, which are stored or pending.
    A chunk that is being generated was never on a screen, with one exception: its
    note was lost in a crash. An action on such a chunk still waits for it, alone,
    by the rule that is there for that (`waits_for_its_chunk`,
    `worker/src/lib.rs:2417-2452`), and keeps its link's later messages behind it
    until the one chunk is made.
  - **This is a step of its own, W5h, in the runner, and it is not delegated**: it
    touches what an edge's resume rests on. Tests from this section by someone
    else: a hello that names a chunk the store is generating is welcomed and its
    link's next input reaches a tick while the generator is held; an input that
    digs in that very chunk, sent again after a restore, does not reach a tick
    until the chunk is loaded, and then takes effect; a chunk that is stored holds
    the link as today.
- **At the edge** nothing waits for a load but the players who are to be sent the
  chunk. A snapshot comes when the chunk is loaded.

**`Unreadable`, and how it ends.** A load is answered `Unreadable` when a stored
chunk cannot be read, as today, or when the generator failed for it, which after
section 5 means a failure in terrain or in starts. The region goes on holding the
chunk without it; a subscription to it is answered with silence; a block of it
"is not there" and nothing can be placed or broken in it
(`crates/clustine-sim/src/region.rs:934`); a player sees no chunk there and can
walk into the gap. What a client shows of that, and of joining inside one, was not
checked.

Today nothing ever takes a position out of the runner's set of unreadable chunks
(`worker/src/lib.rs:766`, `1083`). **It is no longer for ever:**

- **The runner forgets a position when its region gives the chunk back**, and asks
  again when it is granted the chunk anew. (A step of W5, in the runner; it does
  not touch the resume.)
- A runner that is made anew, after a merge, a split, a move or a restore, starts
  with an empty set, as today (`worker/src/lib.rs:868`), and asks.
- The scheduler remembers a failed task for as long as its process runs
  (section 5). So while the store's process runs, asking again gives the same
  answer, at no cost. **When the store is started anew with a generator that no
  longer fails there**, every handle is lost, every region is restored with a new
  runner, every chunk is asked for again and is made. Nothing of a hole is stored,
  so nothing is in the way.
- For a stored chunk that cannot be read, the same asking again finds the file as
  it is then.

**Restore, in full** (`chunks.rs:490-521`, `apply` at `559-594`):

1. it collects the chunks its changes lie in and looks each up;
2. it asks the generator for every one that is not stored and waits for all
   (rule 6);
3. a chunk that cannot be read, or that the generator failed for, is skipped, and
   its changes are not applied, as today for the first (`chunks.rs:578-584`). **It
   is logged at the level of an error, with the chunk and the number of block
   changes that are lost**: the next checkpoint cuts the log, and those changes
   are then gone. Today that needs a damaged file; now a failure in terrain can do
   it;
4. it applies the changes in order and saves the chunks, which are no longer "as
   generated".

With S2 and S5, the chunks a restore has to generate are those that were handed
out and not yet durable when the process died, and that were changed since: few.
W5 times a restore after a kill with nothing cached, on the imported world,
against M3's "5 to 7 seconds".

`carry_over` (`local.rs:76-133`) cannot run on a world of format 2 (ADR-0022) and
is not touched.

**The new text of ADR-0017's test T10**, item 1
(`docs/adr/0017-the-end-of-the-stripes.md:2266-2277`). Its head and items 2 to 6
stand, with "a generator of chunks that the test can hold" read as: its `request`
says when it is entered for a chunk, and calls `done` when the test lets that chunk
go; it remembers nothing between requests; and the test holds the only handle to
it besides the store's. `X` and `Y` are handles of two regions.

> 1. *The thread for chunks is waited for, and so is the generator.* `X` is opened
>    and commits three ticks. Then, without waiting for any answer: a load of a
>    chunk `A` that was never stored, which enters the generator and is held there;
>    a checkpoint of the third tick; and the handle is dropped, as a runner that is
>    abandoned drops it. The barrier is asked for and not waited for. The list of
>    regions is read and answered, so the commit thread has had the barrier's turn.
>    **The barrier is not answered**, and a store started on what a crash would
>    leave now restores the region with three commits and no state of the third
>    tick, and has no chunk stored at `A`. `A` is let go. The barrier is answered
>    `Ok`. **A store started on what a crash would leave at that moment, with
>    nothing that was not durable, restores the region from the state of the third
>    tick and no commit, and has `A` stored, equal to what the generator gave and
>    marked as generated by its version.**
>
>    1a. *Loads of other chunks do not wait.* `X` loads `A`, held, and then `B`,
>    never stored, which enters the generator while `A` is held. `X`'s queue has
>    `Generating` for `A` and for `B`. `B` is let go: the queue has `Loaded` for
>    `B`. `A` is let go: `Loaded` for `A`. And: `X` loads `A`, held, and then a
>    chunk `S` that is stored: `Loaded` for `S` is in the queue while `A` is held.
>
>    1b. *Loads of one chunk keep their order, and a save keeps its place.* `X`
>    loads `A` twice: the generator is entered twice or once; after `A` is let go
>    the queue has `Loaded` for `A` twice, with equal chunks, and one chunk is
>    pending for `A`. `X` loads `A`, held, then saves a chunk `C` that was never
>    stored, changed in one block, then loads `C`: nothing of `C` is answered while
>    `A` is held and the generator is never entered for `C`; when `A` is let go the
>    queue has `Loaded` for `A` and then `Loaded` for `C` with the change in it.
>
>    1c. *A failure.* The generator fails for `A`: `X` is answered `Generating` and
>    then `Unreadable` for `A`; nothing is stored at `A`; a later load of `A`
>    enters the test's generator again.
>
>    1d. *A lost handle.* `X` is lost while `A` is held: the generator is told that
>    nobody waits (the request is dropped); nothing more is answered; nothing is
>    stored at `A` after a flush.
>
>    1e. *Another region does not wait.* `X` loads `A`, held. `Y` loads a stored
>    chunk and is answered; `Y` loads a chunk `D` that was never stored, which is
>    let go at once, and is answered; `Y` saves a changed chunk, asks for a
>    checkpoint and for its own flush: `Y`'s queue has `Flushed`, and what a crash
>    would leave has `Y`'s state file and its chunk, **all while `A` is held**. `Y`
>    returns a chunk and the list of regions no longer has it as `Y`'s.
>
>    1f. *The barrier waits for all.* `X` loads `A`, held; `Y` has asked for
>    nothing. `Store::flush` does not return until `A` is let go.
>
>    1g. *A restore waits for all and keeps nobody.* `X` loads `A`, held. A region
>    `Z` whose commits change a chunk `R` that was never stored is opened: the
>    hello is not answered while `A` is held. `A` is let go; the generator is
>    entered for `R` and held: the hello is still not answered, and meanwhile a
>    checkpoint of `Y` is answered. `R` is let go: the hello is answered, and `R`
>    is stored with the change in it and is not marked as generated.
>
>    1h. *A restore over a request under way.* `X` loads `A`, held, and is lost.
>    `X`'s region is opened again with commits that change `A`. The first request
>    was dropped; the generator is entered for `A` again; when it is let go the
>    hello is answered and `A` is stored with the change. The lost handle's queue
>    has nothing after `Generating`.
>
>    1i. *Withdrawal.* `X` loads `A`, held, and returns `A`. The generator is told
>    that nobody waits; `X`'s queue has `Generating`, then `Withdrawn` for `A`; the
>    list of regions no longer has `A` as `X`'s; nothing is stored at `A` after a
>    flush. `X` claims `A` again and loads it: the generator is entered again, and
>    the `Loaded` that follows is taken by a runner as the answer to its latest
>    load.
>
>    1j. *Stopping with a request under way.* `X` loads `A`, held. `Store::close`
>    does not return while `A` is held; `A` is let go; `close` returns, and what a
>    crash would leave has `A`. And: `X` loads `A`, held; the store and every
>    handle are dropped; `A` is let go or the generator is dropped: both of the
>    store's threads end, and the count of what was done to the disk is what it
>    was.

Item 3 of T10 ("nothing is written afterwards") holds as it is: `flush` syncs
nothing, a note that is pending when the store is dropped is not written, and the
thread for chunks drops its generator last, which is the last one where the test
holds none.

### 8. The generator's settings in a world's `meta`

```text
format=2
data-version=5023
generator=clustine;dimension=minecraft:overworld;seed=13579;structures=blocks;order=nine
generator-version=1
```

- `generator` is `GeneratorSettings::identity`: the kind of generator; the
  dimension (a world has one, the plan's question 6); the seed as a signed decimal
  number of 64 bits; `structures=blocks` or `none` as ADR-0019, section 4, has it;
  the order of runs, `nine` or `alone`. The flat generator's is what it is today
  (`services/worldgen/src/lib.rs:58-70`). **It must be equal**, or the store stops
  with `StoreError::Incompatible` as today (`local.rs:44-58`).
- `generator-version` is the highest version that made a chunk the world stores,
  by section 6. It is a line of its own, and today's reader of `meta` passes it
  over when it looks for `generator` (`local.rs:49-52`: a line that begins
  `generator-version=` fails the second `strip_prefix`). A `meta` without the line
  is a world of format 1, which is refused before the line is looked for
  (ADR-0022). No world of the flat generator exists that this server opens, for
  the same reason; the flat generator's version is 1.
- **ADR-0019 is edited in the commit that builds this**: its sentence "The
  `generator` line of `meta` carries `structures=blocks` or `structures=none`
  beside the seed and the generator's version" loses "and the generator's
  version", and "which ADR-0020 decides" becomes "which ADR-0021 decides"
  (`docs/adr/0019-data-made-from-mojangs-jar.md:425-429`). ADR-0020 became another
  record (one stay per player); this is the one it meant.

**The version** is a `u16` constant of `services/worldgen`, one for the real
generator and one for the flat one. **What raises it**: any commit after which any
chunk of any world comes out otherwise, its blocks, biomes, block entities, ticks,
marks or light. That can come from a change to

1. `services/worldgen`: the order of runs, the view, what `finished` and `Lit`
   put together;
2. `clustine-terrain`, `clustine-features`, `clustine-structures`;
3. `clustine-noise`;
4. `clustine-light`;
5. the generated code and tables of `clustine-worldgen-data`;
6. the tables of `clustine-data` that generation reads: the per-state table, the
   block classes, the biome parameters (ADR-0019, section 1);
7. the pinned jar, whose templates are read when a server starts. (A new jar is a
   new `data-version` too, and a world of another data version is refused.)

A fix that makes a placement no longer fail raises it as well; that is what lets
S8 make the marked chunks again.

**What guards it.** The fixtures of the generation workflow (the plan, section 4)
are committed with the version they were made at. That workflow **fails when any
fixture's hash differs and the version is the one the fixtures name**; it is
required for every push that touches one of the seven, as the plan has it for
pushes that touch generation. In the ordinary tests, test 9 does the same for a
few chunks, so that most slips are caught before a push. The version is raised
from the first commit of phase T that the owner walks a world of; before that
every world is thrown away with the next step.

### 9. How exactness is tested

What judges, per stage, is the plan's section 4 with the audit's rules. Both
references are proven: G1 (terrain from the unmodified server under generated data
packs, 14 of 14 chunks equal) and G2 (the server inside a Java program, 100 of 100
at both stages). G2 did not fail, so the fallbacks the plan wrote for that case are
not needed and nothing here is "implemented but not verified" for want of a
reference.

| Stage | Judged by | Rule for the harness |
|---|---|---|
| Starts as made; biomes; terrain; fill alone; fill and surface | region files of the **unmodified official server** under data packs made from its own files (without features; without carvers; with an empty material rule): chunks left at `minecraft:terrain` | single forced chunks 5 apart, ticking frozen |
| Feature runs: blocks, block entities, ticks, marks | **the Java program that runs the server inside itself**, asked for chunks in the order of section 2 | below |
| Piece state | not by itself: through the blocks that later runs place by it | |
| Light of generated chunks | the same program, at the light status, for chunks whose neighbours are lit | nothing lit before the last run |
| Finished chunks of an unmodified world | a count, not an equality (order of runs, post-processing) | the owner's machine |

**The harness rule** (audit, section 6), which F0 builds and which this record
makes part of the definition of "the official server in Clustine's order":

1. Terrain is present for every chunk within 2 of every run before the first run.
2. All runs of rank 0, then all of rank 1, and so on; within a rank any order. F0
   runs each fixture area in two orders within a rank and requires equal hashes
   **for every chunk but those within 2 of a run for which Clustine's recorder
   counted a far read**: there the official server reads a chunk that a run of the
   same rank may or may not have written yet, so its own two orders can differ.
   Such chunks are listed and compared by a count of blocks. A difference anywhere
   else is a breach of condition 1 or 2 of section 2 that the audit missed.
3. **Chunks stay below `full`** (asked for at `features`) until the comparison is
   taken: a worldgen height asked of a full neighbour is answered from live blocks
   by the official server.
4. **No save and reload** between terrain and the comparison: a chunk read back
   from disk has its worldgen heightmaps made again from the blocks it has then.
5. Only chunks whose `R(e)` lies inside the decorated rectangle are compared.
6. **Ticks and marks of unfinished chunks are part of what is compared.** F0 shows
   first that they come out: the plan has the world saved after the last run and
   read back through the one reader, and whether the official server writes the
   scheduled ticks and the post-processing marks of a chunk that is not `full`
   into its file was not checked. If it does not, the program reads them from the
   chunk in memory.

**Tests of this record**, written from it by someone who does not write the code.
Tests 1 to 6 need no terrain and no jar and run in CI's ordinary run.

1. **The order against the plain model** (section 2), with a toy `Stages`: terrain
   is a function of position that fills columns to a height; toy features are
   seeded by seed and centre, read blocks and final heights at positions up to 16
   blocks beyond the centre chunk, write up to 16 beyond, read worldgen heights
   and biomes up to 32 beyond, leave ticks and marks, and set the state of toy
   pieces whose boxes span chunks at most 2 apart, **some of them twice in one
   run**. Over a rectangle of at least 30 by 30 chunks, for both settings of
   `order`, every chunk whose `R(e)` lies inside equals the model's; with the model
   run in two orders within each rank. Further toys, each alone: one that reads
   live at 17 blocks (answered from terrain in the model's view too, and counted);
   one that writes, schedules and marks at 17 (refused, dropped, counted); **one
   that sets, and one that reads, a piece whose box spans chunks 3 apart** (kept
   and answered, and counted as a far piece and a far piece read).
2. **The order of work does not show**: the same requests on one thread, on eight,
   and with ready tasks taken in a scrambled order (a seeded shuffle the test
   gives) yield equal chunks.
3. **The cache does not show**: with a bound of 1 MiB, of 0, and without a bound,
   the same chunks; with a bound of 0 nothing a request needs is dropped while it
   is under way (the request ends).
4. **The sets of section 2**: for each of the nine classes, one `finished(e)` from
   nothing makes exactly the 94 runs of `R(e)`, the 289 terrain of `T(e)` and the
   1,089 starts within 8 of those; one first view makes counts between the least
   and the most of the table, starts among them.
5. **Look-ahead and cancelling**: a request's eight neighbours are in the cache
   afterwards and a request for one is answered without a task being run; a
   dropped request leaves no task that has not started; a task under way when its
   request is dropped ends and its result is used by the next request.
6. **Failures.**
   - A toy placement that panics after it wrote blocks, a tick, a mark and a piece
     state: none of them is in any chunk; the placements before and after it are;
     the chunk equals that of a generator in which that placement does nothing and
     draws nothing from what the others use.
   - Exactly the chunks whose `Lit` depends on that run carry "with a failure": 25
     for a run of rank 8, 289 for one of rank 0; no other chunk does.
   - A panic between two attempts cuts the run short and keeps what was placed.
   - The log line has the seed, the chunk, what was placed and the version.
   - A toy `terrain` that panics at one chunk fails exactly the requests the table
     of section 5 gives for its class (289 to 400), and a toy `starts` 1,225; the
     same request again runs nothing; a new scheduler runs it again.
7. **The store** (W5), on the disk in memory of the store's kill tests:
   - T10 as above, 1 to 1j.
   - S1 to S8, each: a generated chunk is in the files after the next checkpoint
     of **any** handle and not before; after 512 notes without one, and not after
     128; marked with the version; a chunk made ahead is not; a kill before the
     sync loses the note and the next load gives an equal chunk; **a sync that
     fails loses the handle that saved and keeps the notes, which the next sync
     writes**; `close` writes what is pending and `flush` does not.
   - **Two regions and one chunk**: generated for `X`, returned by `X`, claimed,
     changed and saved by `Y`, returned, claimed and loaded by `X` again: `X` gets
     `Y`'s chunk. The same with `X`'s first load still held when `X` returns
     (withdrawn), and `Y` then generating it: `Y`'s change is what is stored.
   - A restore whose commits change a chunk that was handed out and never
     durable; a restore over a chunk the generator fails for, which logs an error
     with the number of changes lost.
   - S8: a stored chunk marked with a failure and version 1 is made again by a
     generator of version 2 and the new one is stored; not by version 1; not if
     it was changed; and is answered as stored if version 2 fails for it.
8. **A newer generator**: a world made with version 1 and opened with 2 has
   `meta` at 1 until a chunk is noted and synced, and at 2 from that sync on;
   what a crash would leave never has a chunk file of version 2 beside a `meta`
   of 1 (the sync is failed at every point in turn); opened with 2 and closed
   without a new chunk, it opens with 1 again; with `meta` at 2 it does not open
   with 1, with the sentence; a chunk stored before is as it was and marked 1;
   another identity is refused as today.
9. **The version is raised**: hashes of a few chunks of the real generator and of
   the flat one are committed beside the version; the test fails with a sentence
   when a hash changed and the version is the same. (The guard that is relied on
   is the workflow of section 8.)
10. **Fixture runs record nothing** (phase F): every run of every fixture counts no
    refused write, no dropped mark, no read beyond the margin, no far piece and no
    failure; far reads and dropped ticks only while a geode is being placed; far
    piece reads only for starts that S2 has read and listed. All are summed and
    written into the parity matrix.
11. **The runner** (W5): a tick's loads go out nearest the region's players first;
    a position is taken out of the set of unreadable chunks when the chunk is
    given back, and is asked for again when it is granted again; `Withdrawn` is
    counted as an answer; the three cases of W5h (section 7).

**Figures that fail a measurement**, all on the owner's machine, written into this
record when they are measured. None is asserted in CI, by the project's rule that
tests do not wait on a clock.

| Measured | By | Fails above |
|---|---|---|
| The crowd's wait at a merge and at a split while a third region's first view is generated from nothing | W5, `crowds.rs` | 1 s at worst (ADR-0017, 9.7), or 0.2 s more than without the third region |
| The same for two regions whose players both walk outward | W5 | 1 s at worst |
| A sync of 512 notes of real terrain | W5 | 250 ms |
| A restore after a kill with nothing cached, on the imported world | W5 | 7 s (M3's promise) |
| A join far from anything stored: the first chunk | W5, T8 | 2 s |
| A region's tick while a first view is generated in the same process | T8 | 50 ms |
| The first chunk walked beyond the rim after a start (the cache is empty) | W5 | reported, with the time of a step that finds its neighbours cached beside it |
| The store's end with requests under way (test 7j) | W5 | reported |

### 10. Determinism

- A task's result is a function of its inputs: no clock, no thread identity, no
  random number that is not seeded from `G` and a position, no I/O. Structure
  templates are read from the operator's jar before the first task (ADR-0019) and
  are part of `G` (`structures=blocks` with the pinned jar).
- **A failure is a result too.** A panic is caught where it is thrown, on the
  thread that runs the task, and what was undone is a function of what was done.
  The workspace does not build with `panic = "abort"` (no profile sets it), which
  catching rests on. A stack that overflows or memory that runs out is not a panic
  and ends the process.
- No hash map or hash set of Rust's in anything a result depends on. Where the game
  iterates a Java hash set, the port iterates its own copy of that order (the plan,
  F1), which is a function of the insertions.
- Floating point as the game does it: the operations of `clustine-noise`, which T1
  proved bit for bit; no `mul_add` and no reordering; `exp` in the beardifier
  settled as the logarithm was (the plan, T1).
- The number of threads, the cache's bound, look-ahead, cancelling, and which
  request came first change when a result exists, never what it is (tests 2, 3,
  5). The order in which the store answers loads of different chunks changes
  nothing a region computes: a region takes what was loaded as an input of the
  tick in which it arrived, as today.
- `crates/clustine-sim` does not call the generator and is not touched by this
  record.

### 11. The steps this adds to the plan's W5

| Step | What | Where | Delegated? |
|---|---|---|---|
| W5a | The scheduler with toy stages: tasks, pool, cache, look-ahead, cancelling, `attempt` and failures | `services/worldgen`, `clustine-world` | yes; tests 1 to 6 by someone else |
| W5b | `ChunkGenerator::request`; the thread for chunks by section 7's table; the two answers; notes, their limit and what a failed sync keeps; `Store::close`; S8 | `services/worldstore`, `clustine-rpc` | yes, on its crates; T10 and test 7 by someone else |
| W5c | `generator-version`: written when a world is made, raised in the sync; the refusal; ADR-0019's sentence; `world-format.md` | `services/worldstore`, docs | yes |
| W5d | The runner: loads nearest first; `Withdrawn` counted; the set of unreadable chunks forgets | `services/worker` | yes; test 11 |
| **W5h** | **The hold of a link and `Generating`** | `services/worker` | **no** |
| W5m | The measurements of the table above, into this record | | the main session |

W5b needs W5a's trait only. W5d and W5h need W5b's answers.

## Ruled out

| What | Why not |
|---|---|
| Shape A: staged generation with state, as the official server | The world would depend on who asks first; unfinished chunks would have to be durable and ordered across crashes; a store with replicas would have that state to replicate; the differential tests would no longer compare equal worlds (the plan, section 1) |
| Shape B as what is built | Wrong wherever two chunks' features meet, and it cannot be proven by hash at all. Kept as a switch for measuring |
| A wider dependency and an order within a class for the desert pyramid (the plan's section 1) | The audit: its scan stays in the three by three |
| Sixteen classes, to isolate reads two chunks out | Nothing reads live two chunks out but the geode's one block, which is rarer than one in a thousand geodes by estimate; chains twice as long |
| A view that panics on a read beyond the three by three | The geode would bring a run down. Counted instead, and the fixtures must count nothing else |
| The three by three alone as the frozen view | Saves 11 % of the terrain of a first view and leaves the geode's read unanswered. The margin stays |
| The "long" ranking (reach 2, 4, 8, 10) | 1.2 % fewer runs for a first view, and a third more when walking along x. The even one treats the axes alike (audit, section 7) |
| A cache of chunk states | A chunk has nine states on its way; none can be dropped alone |
| A cache on disk for intermediate results | The plan's review, finding 10: a second set of files that can be torn. The store's own files hold what was handed out, whole or not at all |
| A sync of its own for a generated chunk; or an answer that waits for one | Every first sight of a chunk would pay the disk. Purity makes it unnecessary |
| Generating in the worker, or in a service of its own with a wire | Only the store knows what is stored. A service of its own is possible later behind `ChunkGenerator`; nothing here stands in its way |
| **One order for everything the thread for chunks does** (the draft, and the plan's sentence "the chunk thread keeps the order of answers") | The review's finding 1: a merge's checkpoint, and every other region's loads, returns and restores, would wait behind any region's generation, without a bound |
| An order per handle that holds a handle's loads of other chunks back too | Players of one region would wait for each other's generation: one who walks in stored land behind one who flies outward. The worker needs the order by chunk only |
| **A failure that takes its whole cone out of the world** (the draft; the plan's "answered as `Unreadable`") | 25 to 289 chunks for one tree that panics, void that a player can walk into and not act in, for as long as that binary runs. Kept only where nothing can be undone piece by piece |
| A failed run taken as a run that did nothing | It throws away the placements that worked. The attempt is the unit |
| A failed `Starts` taken as "no start there" | Terrain is shaped for starts: it would change 1,225 chunks without a hole to show it |
| Withdrawing a load without telling the region | The worker counts answers by position; its count would stay one too high and it would throw the next answer away |
| A message with which a region takes back a load of a chunk it still holds | Not needed while giving back withdraws. T8's flight says whether thirty seconds of unwanted loads are too many |
| Writing the newer version into `meta` when a world is opened (the draft) | Trying a newer build once would shut the older one out |
| Keeping old generator versions in the binary (the plan's question 2c) | The owner chose 2a |

## Risks

- **A live read beyond the three by three that the audit missed**, a piece that
  keeps state over more than two chunks, or a start whose first piece moves far
  from a piece that reads it. The audit read SteelMC's port, not Mojang's code.
  The recorder counts all three in every fixture run, and the harness's two orders
  within a rank would show a difference in the official server itself.
- **A failure in terrain or in structure starts is a hole of 289 to 1,225
  chunks**, until a server with a generator that does not fail there is started.
  The port carries a `todo!` and many `panic!`s (the plan's check, section 2).
  Fixtures of terrain and of starts over wide areas come before the owner walks
  such a world; they cannot cover every seed.
- **A failed placement leaves a world that is not the official one**, in the
  chunks that are marked, until a newer generator makes them again, and for good
  where someone changed them first. The count of failures is logged; the parity
  matrix says that a world with a failure in it is not block for block there.
- **A region waits for its own generation at a merge or a split** (section 7).
  Measured in W5; choice 2.
- **Loads nobody wants any more** are generated, answered, noted and written for
  up to thirty seconds after the region stopped wanting them, and the cache's
  bound does not bind while they are under way. A player who flies at 1.35 chunks
  a second asks for about 26 new chunks a second; if the pool is slower than
  that, the queue grows for as long as the flight lasts and the chunks where the
  player is now come late, since the pool works for the oldest request first. T8's
  flight measures it. What would be built then: a message to take back a load of
  a chunk that is still held, and the pool working for the newest view first.
- **Every chunk a region was answered with is written.** A sync of up to 512 chunk
  files keeps the thread, and every region's jobs with it, for as long as it
  takes. ADR-0022 makes a chunk one file for this reason. The store's and the
  end-to-end tests see more writes than before, on the flat world too.
- **The thread for chunks is no longer a queue.** What waits for what is a table,
  and a mistake in it is an ordering bug of the kind that hides. T10's ten cases
  and test 7 are written from the table by someone else; the store's existing
  scenarios must pass unchanged, since for a generator that answers at once the
  table gives today's answers but for `Generating`.
- **`Generating` lets input through that the hold kept back.** W5h's argument is
  that such a chunk was never on a screen but after a lost note. It is not
  delegated and has tests of its own; it is the part of this record nearest to the
  edge's resume.
- **The cache is too small or too large.** 512 MiB is a guess from guessed sizes.
  Too small shows as runs made twice (the scheduler counts runs made and runs made
  again; W5 reports both), never as a wrong chunk.
- **The pool starves the regions' ticks** in the single process. T8, with the
  figure of section 9.
- **C is too slow** (the plan's risk). T8 measures; `order=alone` is the switch,
  for a new world.
- **The seam, and the crash followed by a newer generator**, section 6.
- **The version is not raised when it should be.** The workflow of section 8 is
  the guard and is required; a change that shows in no fixture passes it.
- **`PieceState` is too small** for some piece, or a field the port calls per-run
  is read by a later run after all. G5 reads the port before it is fixed.
- **Restore loses changes** to a chunk the generator fails for, with an error in
  the log and nothing a player is told.

## Not checked

- Nothing was built. No time, no size of a result, no size of the cache is
  measured: W5 and T8, against the figures of section 9.
- How often the geode's case happens, and whether the fixtures see any other far
  read or a far piece read (phase F, S2).
- Mojang's code. Conditions 1 to 3 of section 2 are the audit's reading of
  SteelMC's port and of the jar's data.
- What `update_shape` and `can_survive` of the block behaviours read in the two
  shape passes (the audit could not determine it; the recorder would show a breach
  only if it lands outside the three by three).
- What the view answers above and below the dimension, and for a final height of a
  chunk in the margin: the port's rules, taken over with it.
- Whether the starts within 8 of 1,337 chunks (2,777 source chunks for a first
  view) cost anything to speak of. Most hold nothing (**guess**).
- Whether the port's placements keep to "nothing outside the view but locals and
  the random source handed in", which `attempt` rests on. F1 reads the runner for
  it.
- That the official server writes ticks and marks of unfinished chunks into its
  files (harness rule 6).
- What a real client shows where chunks do not come, and when it leaves its
  loading screen.
- How long the hold of a link lasts in play today, and that nothing but the hold
  and `waits_for_its_chunk` rests on a chunk being loaded when a hello is
  answered. W5h reads the runner for it.
- How long a sync of 512 chunk files of real terrain takes on the owner's disk.
- Whether T8's spawn area is to be stored when a world is made. No region was
  handed it, so S6 says no; T8 decides.
- The Nether and the End: the audit covered their placed features, not how the
  End's fixed features are driven. Phase D.

## Choices the owner has not settled

Each is decided above so that the record can be built from; each can be turned.
Those the session decided after the review are not listed again (the order per
region, the hold, failures caught at the placement, the sync at a stop, when the
version is raised).

1. **A failed `Starts` or `Terrain` leaves a hole** of 1,225 or 289 to 400 chunks
   until a fixed server is started, where the other way is to take a failed start
   as no start and shape the land without it.
2. **A region's checkpoint waits for the region's own loads under way.** Nothing
   needs that but the wish to keep one order per handle; a checkpoint that waits
   for saves only would take a region's own generation off a merge's standstill
   too. W5 measures first.
3. **A world opens with a newer generator even after a crash**, with the limit of
   section 6. The other way: `meta` records that the world was stopped in order,
   and a newer generator is refused until the older one has started it once more.
4. **A handle's loads of different chunks are answered as they come**, not in the
   order asked. The session said that a waiting load holds back only later jobs of
   its own handle; this holds back fewer still.
5. **Which chunks a failure marks**: every chunk whose light depends on the run,
   25 to 289, rather than the nine the run could write into.
6. **A failure is recorded in the log and in a count**, and in the world only as
   the mark; there is no file of failures.
7. **A chunk with a failure is made again when it is next loaded** under a higher
   version, not by a pass over the world when the version rises; and it is left
   alone for good once anyone changed a block in it.
8. **`generator-version` as a line of its own** in `meta`, and ADR-0019's sentence
   edited to fit.
9. **Notes have a limit of their own, 512**, and are kept when a sync fails.
10. **The figures of section 9**: 1 s and 0.2 s for the crowd, 250 ms for a sync,
    2 s to a joiner's first chunk, 50 ms for a tick, 7 s for a restore.
11. **The even ranking**; **the frozen margin stays**; **a read beyond the five by
    five is answered from terrain fetched on demand** and counted.
12. **A piece's state travels piece by piece**, as a shift in y, a settled flag
    and sixteen flags, until G5 has read the port.
13. **The pool**: threads of its own, two thirds of the processors by default.
    **The cache**: 512 MiB, never dropping what a request under way needs.
    **Look-ahead**: the eight neighbours of every chunk asked for, kept in memory.
14. **Saving behind the answer holds for every generator**, the flat one too.
15. **`ChunkGenerator` keeps a blocking `generate` that panics on failure**, beside
    `request`, so that the tests that call it stay as they are.
16. **`order=alone` (shape B) is a setting of a world**, part of its identity.
17. **The harness runs every fixture area in two orders within a rank.**

## Review

An independent reviewer went over the draft against the code at `b67a1d0`, the
audit and SteelMC's branch, and recomputed the lattice. Eleven findings. Each was
checked against what it cites before it was taken.

1. **One order for the whole thread puts every region behind any region's
   generation, without a bound.** Accepted; confirmed: one thread and one queue
   (`chunks.rs:366-370`), the second checkpoint of a merge on it
   (`worker/src/lib.rs:2219-2229`), the worker's count by position
   (`:1138-1161`), holders only (`lanes.rs:680-689`). The session decided the
   order per region; section 7 has the table of what waits for what, the proof
   for S1 to S8, the rule for a merge's checkpoint with its test and its bound,
   and T10 anew with cases for two handles. Notes got a limit of their own, as
   the finding asked. **Gone further than asked** in one point: a handle's loads
   of other chunks do not wait for each other either (choice 4). **Kept against
   it** in one: a restore waits for all, by the session's decision, but does not
   keep the thread.
2. **A link is held until every chunk of its hello is loaded, and loads now take
   seconds.** Accepted; confirmed at `worker/src/lib.rs:2745-2761`, `2374-2403`
   and in ADR-0012, 4.5; loads go out ascending (`region.rs:1633`). Section 7:
   what a joiner and the others wait for, loads nearest first, `Generating` and
   step W5h, which is not delegated; the first chunk is timed.
3. **A panic's hole is larger and more lasting than the draft said.** Accepted;
   confirmed that nothing takes a position out of the runner's set, and the
   sizes for terrain and starts recomputed (289 to 400 and 1,225 lit chunks). By
   the session's decision a failure is caught at the placement and undone, the
   chunk is marked and made again under a higher version; terrain and starts
   stay `Unreadable` with their sizes; the runner's set forgets. **Rejected**:
   taking a failed start as "no start there" (the finding left it open): terrain
   is shaped for starts. It is choice 1.
4. **The counts of starts were of another set than the one defined.** Accepted;
   recomputed: 2,601, 2,777, 2,862 and 2,907, 2,993, 3,141; 1,089 for one
   finished chunk. Table, section 5 and test 4 corrected.
5. **The geode makes two equalities false as written.** Accepted. The plain model
   is said to be the official server but for far reads; harness rule 2 excepts
   chunks near a counted far read.
6. **Loads nobody wants cost work, memory and disk without a bound.** Accepted in
   substance: a return withdraws a waiting load (rule 5), S6 says what "shown"
   is exactly, and the rest is a risk with what T8 would have built. **Rejected**:
   that it needs no new message. The worker counts answers by position
   (`worker/src/lib.rs:1149-1161`), so a load that is never answered would leave
   its count wrong; `Withdrawn` is that answer.
7. **A save of any chunk stopped the pool from seeing the loads behind it.**
   Accepted; the rule is stated by what can change the answer (the table: a job
   that names the same chunk, a restore, a fold).
8. **S7 had two holes on an ordinary stop.** Accepted; confirmed at
   `worker/src/lib.rs:2238-2240` and `chunks.rs:236-239`. `Store::close` syncs,
   `flush` stays as it is so that T10's item 3 stands, and a failed sync keeps
   the notes (S5, S7).
9. **Versions and `meta`.** Accepted, all three doubts: the list of what raises
   the version and the workflow as the guard that is required; the version is
   written when a world is made and raised with the first chunk stored; no flat
   world of format 1 is opened. ADR-0019's sentence is edited in the same commit.
10. **What the tests would miss.** Accepted: two handles (T10 1e to 1g, test 7),
    a restore over a request under way (1h), a stop with requests under way
    (1j), figures that fail (section 9's table), ticks and marks of unfinished
    chunks (harness rule 6), a far piece and a piece set twice (test 1), and
    that a breach of condition 3 has no count.
11. **Left unsaid.** Accepted, each: the cache is empty after a start (section
    5); the pyramid's extra state, confirmed in the port
    (`S263:steel-worldgen/src/structure/desert_pyramid.rs:18-27`), is G5's to
    keep local to a run; the start's reference position is named as a read of
    state and counted (`S263:…/feature/runner.rs:319-321`); `Wanted` is defined;
    the thread needs more than one way to wait and the record no longer suggests
    one; the generator is made once for the store alone; the pool against the
    runners is measured with a figure; a restore's lost changes are an error
    with their number; "finished" is before post-processing. **Not done**: making
    the pool give way to the runners. Nothing portable does that; the share is
    lowered instead if the ticks suffer.

Nothing of the review was rejected outright. Three of its proposals were answered
otherwise than proposed: withdrawal has an answer of its own (6); a failed start
is not "no start" (3); and the global order was not kept with a figure to fail it,
which the finding offered as the second way (1).
