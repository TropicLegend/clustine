# ADR-0029: Light in the simulation

- Status: **Proposed** on 2026-10-26, from the plan of step N8
  ([n8-plan.md](../groundwork/n8-plan.md)), after an independent review of that plan
  whose findings are worked in (its section 14) and the lead's rulings on what the
  revision left (its section 14.x). Where the plan and this record differ, this record
  holds; where the plan's sections and its section 14 differ, section 14 held and is what
  is written here. Not yet reviewed itself. **Partly built** (N8a's first part, on
  `main` at `0b5fa80`): the engine's interface with its first, whole-chunk
  implementation, the noting in `Chunk::set`, the region's light step, the budget of
  arrivals and the rule that a chunk ticks only once its 3×3 is lit, the region's light
  read by mobs and the rules' views. "Built so far" at the end says what is and what is
  not. **The budget of section 6 is the lead's decision, which the owner may
  overrule.**
- Date: 2026-10-26. The code it cites is as at `0b5fa80`.
- It amends [ADR-0023](0023-light-at-the-edge.md) (regions light their chunks too; the
  edge is unchanged; nothing passes between the two), [ADR-0024](0024-block-behaviour-in-a-region.md)
  section 4 (what `Region::restore_loaded` promises) and section 8 (light is now in the
  simulation), and [ADR-0025](0025-effects-between-regions.md) section 4 (`SetIf` for
  growth and trees). "Changes to earlier records" gives the text.
- Marks as in the plan: the game's classes are those of the 26.3 jar, dumped with
  `javap` into `/home/claude/reference/n8-javap/` (cited "n8" with the class and its
  lines) and earlier dumps ("n7"); **guess** where a figure is not yet measured.

## Context

ADR-0022 put a chunk's light into what the chunk carries, made by the generation pool,
and ADR-0023 made the edge keep light up to date for what clients are shown. The
simulation had none. ADR-0024 (section 8) therefore left out every behaviour that reads
light, and the owner's answer to its question 3 made light in the simulation a step of
its own after phase B. Before N8:

| What | Where |
|---|---|
| The only read of light was a mob's: the chunk's carried light, or a fallback of the sky over the motion-blocking heights with no block light at all | `MobWorld::light`, `crates/clustine-sim/src/region/mobs.rs` (before `0b5fa80`) |
| Every rule's view answered "no light" (`BlockView::raw_brightness` gave `None`) | `region/updates.rs`, `placement/context.rs`, `rails.rs`, `physics.rs` |
| The random ticks that need light did nothing: crops, stems, berries, bamboo, saplings, grass and mycelium, mushrooms, snow and ice | `region/random_ticks.rs` |
| Carried light is dropped by the first change light can see (`light_tells_apart`); in a seeded world after B6 most chunks lose it at post-processing | `crates/clustine-world/src/chunk.rs` `Chunk::set`; ADR-0023 section 9 (72 in 100) |
| `clustine_light::light` gives a chunk's light as a function of it and its eight neighbours, an absent neighbour a wall; light reaching a chunk starts at most 14 steps from it, so it stays within the 3×3 | `crates/clustine-light/src/lib.rs` |

So a player could not grow wheat, saplings never grew, snow never melted, grass never
spread, and an undead mob under a roof of leaves burned where the game's does not. N9
(natural spawning) reads what N8 makes.

A region's tick is a function of the region and its inputs (`clustine-sim` is
deterministic: no hash maps, no clock, no I/O), and its chunks come and go with loads,
copies, rings, merges, splits, restores, moves and take-overs. This record decides
where light is made for the simulation, when in the tick, what it is a function of,
what it is not (state, committed, crossing a line), how much of it a tick may make, and
what that does to when a chunk ticks.

## Decision

### 1. Light is the region's, made at one step of its tick

Each region keeps the light of every chunk it has (loaded, its ring, lent, or a border
copy) in an engine of its own, and brings it up to date at **one fixed point of its
tick, the light step**, `Region::light_step` (`region/light.rs`), called from
`Region::tick` (`region.rs`, at line 1059 in `0b5fa80`):

1. the clock and `update_chunks` (loads, lets go, copies' bases, updates and
   withdrawals);
2. edge events, players joining, leaving and arriving, the entering stays, the players'
   actions (`apply_remote`, `apply_input`), which read the last step's light;
3. the sleep check;
4. **the light step**: chunks gone taken out of the engine, chunks replaced whole
   relit, the changes the chunks noted drained into the engine (section 4), the waiting
   chunks taken within the budget (section 6), and a run of the engine; the tick's
   darkening is taken once from the clock;
5. `update_ticking`, in which a chunk ticks only where its 3×3 is lit (section 6),
   `run_seams`, `lay_blast_drops`, `take_arrivals`;
6. block ticks, fluid ticks, post-processing, random ticks, block events, entities,
   block entities and the rest, as before.

**Why there.** Every rule of the tick then reads light current with the blocks as the
tick began, which is what the game nearly always gives a rule in the tick after a
change (its light engine runs between ticks and races the next one; the plan's section
2, rows 1 and 2), and a player's action is read in the tick it is applied. The step is
before `update_ticking` (the review moved it there from after `take_arrivals`; the
lead's ruling 1) so that a chunk lit in a tick ticks in that tick: with the step after
`update_ticking`, every load would start ticking a tick later than before N8, and a
region made by `restore_loaded` could never tick in its first tick.

**What it leaves a tick late**, named: what the tick changes after the step (other
regions' effects in `run_seams` and `lay_blast_drops`, which are a tick late already
(ADR-0025 section 1), block and fluid ticks, post-processing, random ticks, entities) is
lit at the next tick's step. A chunk post-processed in its first ticking tick has that
tick's random ticks read the light of its unsettled blocks. No rule of N8 can tell
within one tick; moving the step after post-processing would put the tick's own block
ticks before light.

Everything that reads light in a tick reads the step's light: random ticks, placing and
using in later ticks, survival in shape updates, mobs, the daylight detector.

### 2. The engine and its invariant

The engine is `clustine_light::engine` (`crates/clustine-light/src/engine.rs`):

```rust
pub trait Chunks { fn chunk(&self, at: ChunkPos) -> Option<&Chunk>; } // None: a wall
pub struct LightAt { pub sky: u8, pub block: u8 }   // the sky's undarkened, the blocks'
pub struct Work { pub chunks_lit_whole: u32, pub adopted: u32, pub cells: u64 }
pub struct Kept { pub light: Light, pub walls: u8, pub taken: (u64, u64) }

impl Lighting {
    pub fn new(sky: bool) -> Self;
    pub fn arrived(&mut self, at: ChunkPos);        // present now; if present already, replaced whole
    pub fn adopt(&mut self, at: ChunkPos, kept: Kept);
    pub fn left(&mut self, at: ChunkPos);
    pub fn changed(&mut self, at: BlockPos);        // a change light_tells_apart sees
    pub fn run(&mut self, chunks: &dyn Chunks) -> Work;
    pub fn at(&self, at: BlockPos) -> Option<LightAt>;
    pub fn light(&self, at: ChunkPos, chunks: &dyn Chunks) -> Option<Light>;
    pub fn kept(&self, at: ChunkPos, taken: (u64, u64)) -> Option<Kept>;
    pub fn has(&self, at: ChunkPos) -> bool;
    pub fn is_lit(&self, at: ChunkPos) -> bool;     // present, lit, nothing noted since
    pub fn is_settled(&self) -> bool;
}
```

`arrived`, `adopt`, `left` and `changed` only note; nothing is lit before `run`. A
change in a chunk that is not present is passed over.

**The invariant.** After each `run`, every chunk the engine has been told of (its
*present* chunks) holds exactly `clustine_light::light` of that chunk and its eight
neighbours as they are present, **an absent chunk being a wall**, with the levels of
every section kept, also those the crate lists as absent (light spreads through them).
A chunk the region has but keeps waiting for its turn (section 6) is absent to the
engine, a wall like any other. Since light reaching a chunk starts at most 14 steps
from it, the greatest fixed point over all present chunks is, chunk by chunk, the
crate's function of each 3×3; so how a run gets there is invisible in the values.
Above and below the sections a chunk keeps, the sky is 15 and blocks 0.

**The crate is the engine's oracle.** The first implementation (the plan's E1, built)
gets to the invariant the plain way: a run lights whole, with `clustine_light::light_with`
asking the levels of every section, every present chunk a noted change can alter, with
all eight of its neighbours (a change that fills or empties a section can change which
of theirs are listed), and every chunk beside one that came, was replaced or went. It
is exact by construction and slow (about 13 ms a change at the mean of real terrain).
The rules, the region's step and every test are built on it. **The incremental
engine** (N8b) then replaces its bodies behind the same interface: a port of the game's
propagation (n8 `LightEngine`, `BlockLightEngine`, `SkyLightEngine`,
`ChunkSkyLightSources`), single-threaded, with plain vectors for queues and the game's
`checkNode` for a check, departures taken first, then checks (all decreases, then all
increases), then arrivals and adoptions one chunk at a time each to its fixed point;
uniform sections filled at once as `flood.rs` fills them. It shares the crate's rules of
a cell (`flood.rs`'s `Cell`, `faces.rs`), so the engine and the oracle cannot disagree
about a block. E1 stays in the tests as the oracle; the property tests are written
against the interface and pass on both.

**Not ported** from the game: its section storages' gating, its visible and updating
maps, and the queued data of loading chunks. The crate floods every section alike, and
so does the engine, so there is one oracle. The game and the crate differ only where
the crate already says they may (`lib.rs`: sky light far under something floating with
no block near). SteelMC's engine was not taken because it is a port of a concurrent
design (windows admitted under locks, queues leased from thread-local pools), most of
which a deterministic, single-threaded region would strip.

**Arrival, departure, replacement.** An arrival is lit whole from nothing (a wall that
becomes blocks can only add ways for light). A departure is the reverse: the departed
chunk's face cells become walls and are decreased through the ported `checkNode`, as
any block that comes to stop light is, which keeps the neighbours' own sky columns and
emitters at the face (the review's finding 4). `arrived` on a present chunk means its
blocks were replaced whole, and is taken as a departure and then an arrival in the same
run, since the old blocks may have lit its neighbours (finding 3).

**Determinism.** `run` is a function of the engine's levels and of the chunks it reads,
and by the invariant its result is a function of the present chunks' blocks alone: two
regions that reach the same present chunks by different histories hold the same light.
`Lighting` holds `BTreeMap`s and vectors; no hash map, no clock, no thread.

**Carried light** (N8e, not built). A chunk arriving with `Chunk::light` is adopted
without relighting only where (1) `made_by` is this crate's `VERSION` and `beside` is a
generator version *g* other than 0; (2) all eight neighbours are present to the engine
(lit before, or taken in the same step) and as generated by *g*; and (3) every section
the light lists as absent lies above every section of the chunk that holds a block.
ADR-0023's test T is not enough here: it passes where a neighbour is missing, and at the
rim of what a region has the region's light must be the light beside a wall. Without
clause 3 the levels of absent sections under an overhang would be unknown. How much it
saves (much in the flat world, little in a seeded one) is measured and written here.

**Memory** (**guess**, measured in N8e): about the 9.3 KB of carried light a chunk at
the mean, more for the absent sections kept, and about 1 KB of column sources for the
incremental engine; 4 to 8 MB for a region at a view of 8 with its ring and copies.

### 3. Light is derived: not state, not committed

Light is no part of `RegionState`, of a commit, of the log or of a saved chunk; a region
lights what it has. `Chunk::light` keeps its meaning (the light the chunk was made
with, dropped at a change light can see), and the region never writes its own light
into a chunk. The region's light (`RegionLight`: the engine, the chunks given to it and
those that wait) is **outside the region's equality**: its `PartialEq` holds always and
its `Debug` gives counts, since between ticks a running region's light is its last
step's and a restored one's is none. A region's clone starts with no light and lights
what it has at its next step, as a restored region does; by the invariant it reaches the
same light. Tests judge light by the oracle (section 10), not by comparing regions.

Nothing is raised: the format, `FEATURES_VERSION` and `clustine_light::VERSION` stay; the
engine changes no value the crate gives. **No world is refused.**

### 4. What tells the engine: the noting in `Chunk::set`

**Every write to a chunk's blocks goes through `Chunk::set`** (the region's set, the
direct calls elsewhere, a copy's update through `Chunk::apply`, the tests' builders). So
the noting is there and not at the call sites: a chunk keeps a list of the positions
whose change `light_tells_apart` sees (the test `set` already made to drop carried
light), **switched on** by the region (`note_light_changes(true)`) and off in a chunk
made anywhere else, so generation, the store and the edge pay nothing. The list is not
serialised and is no part of the chunk's hand-written equality. The light step drains
it (`take_light_changes`: ascending, each position once, since the order of changes
tells light nothing) from every chunk the region has into `Lighting::changed`. **No
write can go round the engine**, and no audit of sites is needed.

The same flag tells the step when a chunk was **replaced whole**: a chunk the engine
knows whose object does not note is one the region has not seen before at that place
(a copy's new base, a loaded chunk become a copy or the reverse), and is relit as a
replacement (`arrived` on a present chunk) **in that step, outside the budget**, so that
no ticking chunk beside it stops. A chunk the region has that the engine does not know
(come since the last step, or every chunk of a region made anew) waits for its turn
(section 6). A chunk the engine knows that the region no longer has is `left`.

### 5. Reads

One set of functions on the region, the game's names, in `region/light.rs`:

| Function | The game's | Read by |
|---|---|---|
| `raw_brightness(pos, darken)` | `getRawBrightness`: `max(sky − darken, block)` | crops, stems, berries, bamboo, survival (darken 0) |
| `max_local_raw_brightness(pos)` | `getMaxLocalRawBrightness`: the above with the tick's darkening; 15 outside ±30,000,000 | saplings, grass, mobs |
| `block_brightness(pos)` | `getBrightness(BLOCK, pos)` | snow, ice |
| `effective_sky_brightness(pos)` | `getEffectiveSkyBrightness` | the daylight detector |
| `can_see_sky(pos)` | sky ≥ 15 | mobs, navigation |

Each answers **`None` where the chunk is not lit**: absent, or present and still
waiting, such as a chunk loaded in this very tick that a player's action reads before
the step. `None` means "not judged", as before N8: seeds go in. `MobWorld::light` reads
the region's light, and its fallback is gone. `RegionView::raw_brightness` and the
fluids' view answer, so every `BlockView` rule judges light; the placement view answers
through a light read of the placement `Blocks` trait, so seeds are refused in the dark.
The views that still answer `None` (the rails', physics') read no rule that depends on
light, and each says so.

**The darkening.** `skyDarken` is set once a tick from the region's clock
(`time::sky_darken`) before the chunks tick. Nothing is relit at dusk: the stored sky
light is the sky's full light, and the darkening is taken at each read.

### 6. Arrivals within a budget, and a chunk ticks only once its 3×3 is lit

**This is the lead's decision at the review (finding 2), which the owner may
overrule.** It is named in the roadmap's "Where the work stands".

**The budget.** A chunk the region comes to have (loaded, copied, in its ring) is not
lit at once: it joins the region's queue of **waiting** chunks, which is the region's and
not the engine's (the engine lights what it is told of, so its property tests need no
budget). Each light step takes from the queue, in ascending order of the chunk's
distance in chunks (the larger of the two axes' distances, in the code) from the nearest
of the region's players as they stand after their actions, then of its position (in a
region without players, by position alone), as many as fit in **256 units, a chunk lit
whole counting 8 and a chunk adopted (kept levels, or carried light by section 2)
counting 1**: 32 chunks lit whole, or 256 adopted, a step. A chunk is taken whole or not
at all; one at the head that does not fit waits for the next step. Departures, changes
and replacements are not budgeted.

**Why counted in chunks, never in time or cells.** A count of chunks is a function of
the region's state and of the order, so the tick stays deterministic, and it is the same
whichever engine is behind the interface (a count of cells would differ between E1 and
the incremental engine). The figures (256, 8 and 1) stand until N8e's measurements set
them (the lead's ruling 3): to the most that keeps a step within 16 ms on the owner's
machine in a release build, at the 0.5 ms a chunk lit whole that section 11 allows.

**Why a budget at all.** Without one, a join, a restore or a take-over lights its whole
view in one step: 400 to 650 chunks at 0.5 ms is 0.2 to 0.3 s in one tick, against the
20 ms section 11 allows a join's tick and half of B8's least pause (0.04 s) at a
restore. With it, a view's 400 to 650 chunks take 13 to 21 steps, nearest first.

**Who has a budget.** `RegionConfig::light` (`LightSettings { budget: Option<u32> }`):
a server's is `LightSettings::SERVER`, 256 units (the worker sets it); a test's is
`budget: None`, every waiting chunk taken at each step, so that a chunk a test loads
ticks in the tick it comes, as before N8. The tests of the budget set one.

**A chunk ticks only once its 3×3 is lit.** `update_ticking` asks of every chunk it
would tick, besides what it asked before, that it and its eight neighbours be lit
(`Region::lit_around`). Rules read light only in and around ticking chunks, at most 10
blocks from them (section 8), so no rule reads a waiting chunk, but for a mob's path,
whose nodes can lie farther: there `None` reads as seeing the sky and cuts the path for
the few ticks until the chunk is lit (named). A player's action may read a waiting
chunk and finds `None`.

**What a player sees.**

- **A join or a flight**: the chunks arriving in a tick wait and are taken nearest
  first; the worker already asks for chunks nearest first, so a joining player's own
  chunks are lit and tick first and the rim of the view last. Water at the rim of a new
  view starts to flow a few ticks after the chunks near the player.
- **A restore, a move, a take-over or a restart of the store**: the restored region has
  no light; all its chunks wait and are taken nearest the players first. The chunks
  around a player go on from the first tick; those at the rim of the view **stand still
  for about a second** (a view of 650 chunks lit whole in about 20 steps), then go on.
  The alternative was the whole region stalling for a fifth of a second or more while
  everything is lit.
- **`Region::restore_loaded`'s promise is amended** ("so that it goes on as the region
  whose state and chunks were taken in the tick before"): it holds where the region's
  chunks fit in one step's budget, and otherwise its chunks wait for their light as
  arrivals do. Every test that relies on it runs without a budget, so the promise holds
  there whole.
- Light at the rim of what is lit is the light beside a wall until the waiting
  neighbour is taken: a lit chunk beside a waiting one is lit as if a wall stood there.

### 7. Merges and splits hand on the kept levels

A merge and a split begin anew through `Region::restore` with warm chunks
(`take_absorbed` and `take_split` replace the region whole, `region/reshape.rs`); the
loaded chunks go back to the runner, which keeps them in `Warm` and hands them back as
`chunks_loaded` only once a ticket asks again, a tick after the request. No `Lighting`
survives that (the review's finding 1: the draft's "the absorber keeps its `Lighting`"
could not be built). So **each chunk's levels go with it**:

1. Before giving its chunks up, `take_absorbed` and `take_split` run the light step once
   more, so that the last tick's noted changes are lit (unbudgeted, as changes are;
   waiting chunks go without levels), and hand each lit chunk out with
   `Lighting::kept`: its levels, which of its eight neighbours were walls (`walls`), and
   which hand-over they are from (`taken`: the region and its tick).
2. The runner keeps the `Kept` beside the chunk in `Warm` and hands it back with the
   chunk in `chunks_loaded` (an `Option` beside each chunk). A `Kept` is dropped with its
   `Warm`; a warm chunk is not written while it is warm, so the blocks are the same by
   construction. A chunk the store delivers afresh has none and is lit as any arrival.
3. The region queues a chunk with a `Kept` as an **adoption** (1 unit), and `adopt`
   mends only its faces: towards a neighbour adopted from the same hand-over in the same
   run they stand; towards any other (one adopted in an earlier step among them, whose
   own face towards this chunk was decreased while this one was absent) they are taken
   as a departure and then, where the neighbour is present, as an arrival, so that
   light is pushed back both ways (the lead's ruling 2).

So a merge or a split relights only faces, a step or two after the chunks come back.
This is **ordering-critical and the lead's** (N8e), not delegated. E1's `adopt` lights
the chunk whole and ignores the kept levels, which is exact; only the cost differs.

### 8. Nothing of light crosses a line

Each region lights its present chunks from its own blocks and its copies'; no light is
sent, received or stored for another region. A chunk a region ticks has its 3×3
present (ADR-0024 section 2, ADR-0025 section 3) and lit (section 6), so its light is
the oracle's for its 3×3: the light a single region would have with the same blocks.
Two coupled regions see each other's changes in their copies at the start of the next
tick and light them at its step, which is the game's own timing for light; an uncoupled
copy is one to a few ticks late (ADR-0025 section 1), and light at the line with it.

**Where the 3×3 is not there** (a copy, a ring chunk), the chunk is lit with what the
region has, an absent chunk a wall, as the crate and the game's engine both take a chunk
that is not there (n8 `LightEngine` answers bedrock). **The rim's bound**: every chunk in
the 3×3 of a ticking chunk is present and lit, so an absent or waiting chunk begins at
least 17 blocks from it along x or z. A read within *d* blocks of a ticking chunk misses
only light whose source is at least 17 − *d* steps away, which arrives with at most
*d* − 2, so it is exact wherever the true level is *d* − 1 or more. Mobs tick only in
ticking chunks (ADR-0028 section 2) and read up to 10 blocks away, where a missing source
gives at most 8:

| Read in a copy or ring chunk | Within | Exact? |
|---|---|---|
| `canSeeSky` (sky ≥ 15) | anywhere | yes: 15 comes only from the column's own sky |
| `FleeSunGoal`'s hiding place (a walk target value below 0, which needs raw light of 12 or 13, by the float) | 10 | yes |
| a mushroom's survival where its spread lands (< 13) | 5 | yes: a missing source gives at most 3 |
| a grass block's spread (no light read at the target) | 1 | — |
| a stroll's ranking of candidates by light | 10 | levels of 8 or less may read lower: a monster may judge one dark corner darker than another |
| a path's trim from the sun (`canSeeSky`) | the path | yes |

So **no threshold any rule or mob of N8 reads differs**; only the ranking of dark
stroll candidates near a line or the rim of a view can. Named. Not taken: the fallback
of sky over heights (the plan's option b, wrong under every roof); copies two chunks
across (option c), which would make the ranking exact at a change of ADR-0025 and many
more copy updates on the stream; and the holder's light on the stream (option d), a
tick late and a new kind of message. Neither is worth the ranking.

### 9. Growth and trees across a line, as `SetIf` effects

What a rule decides from light stays the decision of the region that ticks the plant;
what it then sets in another region's land crosses as blocks, as ADR-0025 section 4's
conditional set (`Effected::SetIf { expected, state, flags }`), `expected` being the
state the rule read there, done by the holder by its rules a tick later when coupled,
or a few when not; a refused one does nothing:

- a **grass block or mycelium spreading** into a copy (one across, up to three down;
  expected dirt), and a **mushroom's** spread (up to five across; expected air);
- a **stem's fruit** (one across). The stem turns to its attached form at once; if the
  holder refuses the fruit, the stem stays turned until the next shape update from that
  side, which turns it back now that `AttachedStemBlock.updateShape` is ported (named);
- a **tree's** blocks (N8f). The region places a feature through a view of its own over
  its present chunks (`region/feature_view.rs`): writes into its own land go through its
  set with the feature's flags (19 for a tree's logs, leaves and decorations) and their
  updates, in the order made; writes into a copy are kept in the view, read back by the
  feature, and become `SetIf`s in the order made when the feature returns, among them
  the existing leaves whose distance the tree rewrites. A write where the region can read
  no block (neither loaded nor copied) is refused, as the region's set drops one. A
  feature placed so reads the light generation's features read (sky 15, blocks 0); no
  tree, mushroom or bone meal feature reads light;
- **bone meal's** features (grass's walks up to about eight blocks out; huge mushrooms;
  a sapling's tree at its second success), through the same view.

Named: a block of a tree refused across a line (a player built there meanwhile) leaves a
gap the game would not, and a leaf refused keeps its old distance until its next update;
a bee nest grown across a line comes **without its bees**, since `SetIf` carries a state
and no block entity. The grower and the feature draw from a `WorldgenRandom` seeded from
the section's stream at the moment the game would start drawing for the tree, so the
same region state gives the same tree; the values differ from the game's, as every
random tick's do (ADR-0024 step B7).

### 10. What a region checks of itself in tests

After the step a region may compare the light of lit chunks with
`clustine_light::light` of their lit 3×3 and panic on a difference. How much is a field
of the light settings of `RegionConfig`, not a cargo feature (a feature enabled through
`bin/clustine`'s dev-dependencies would unify into the whole workspace's test build:
finding 16):

- in the simulation's own tests, every chunk the step touched (lit whole, adopted,
  replaced or reached by a change);
- in the end-to-end tests, the worker reads `CLUSTINE_LIGHT_ORACLE` (a number of chunks
  a step, set by the tests' harness, 0 elsewhere) and the region compares that many lit
  chunks a step in turn, in ascending order from where the last step stopped, so a
  lasting difference is found within a turn of the region's chunks. 4 to begin with,
  chosen so that `tools/check.sh` grows by less than a tenth.

Every existing test of the simulation (water, pistons, TNT, mobs, merges and splits)
thereby tests the engine, the adoptions and the handing of chunks to it; since the
noting is in `Chunk::set`, the oracle is there for the engine and the region, not for
missed writes.

### 11. The figures to be measured

The region never reads a clock: each step counts its `Work` (chunks lit whole, adopted,
cells visited, and the queues' peak) and returns it in its `TickOutput`. Time is taken
outside it: by the engine's benches, by the worker around each whole tick (logged beside
the step's `Work`), and, for the step alone, by a bench that replays the calls a region
made to `Lighting`. Each figure says whether the world was in memory
(`TMPDIR=/dev/shm`) or on the real disk with its cache, in a release build, on a quiet
machine. They are written into this record whatever they are.

| Case | Measured | Acted on if (**guesses**) |
|---|---|---|
| The engine alone (a bench in `clustine-light`) | a torch placed and broken; a glowstone ceiling of 64; a lamp toggled; 64 TNT's crater; 400 chunks lit one at a time as the budget takes them; 400 adopted; each beside E1's time | a chunk lit whole dearer than 0.5 ms; an adoption dearer than 0.06 ms; a torch dearer than 0.2 ms |
| A builder (a bot placing and breaking torches, lanterns and glass twice a second) | the step's time a tick, worst and 99th centile | worst over 5 ms, or 99th over 1 ms |
| A river across a view, and a pinned line (E7's case) | the same, and the coupled pair's ticks a second | worst over 5 ms; ticks a second below E7's |
| 64 TNT (N7f's crater) | the step in the tick after | over 10 ms |
| A lamp clock of 16 lamps (R8's), redstone ore lit by walking mobs | the step's mean and worst | mean over 1 ms |
| A first join at a view of 8, seeded and flat | the step at each tick of the join; chunks carried, adopted and lit; the ticks until the view is lit whole | any tick's step over 20 ms; the view lit whole later than 2 s after its last chunk arrives |
| A restore, a move, a take-over | the pause as B8 and E7 measured it, the first step's share, the ticks until every chunk is lit | the pause longer by more than 20 ms; every chunk lit later than 2 s |
| A merge and a split with the kept levels | the same, and the chunks relit whole | as above; any chunk handed back warm relit whole |
| Memory | the engine's bytes a present chunk and a region's total | over three times the 9.3 KB mean |
| `tools/check.sh` | its length beside the last full check | longer by more than a tenth (then a smaller `CLUSTINE_LIGHT_ORACLE`) |
| Carried light | how many arrivals are adopted by section 2's test, flat and seeded | (written here; no bound) |

"Acted on" means mended within N8 before it is called done: the engine made cheaper,
the budget's figures lowered, or, if that is not enough, a question to the owner with the
figures.

## Consequences

- Crops, stems, berries, bamboo, saplings and the ground propagule, grass and mycelium,
  mushrooms, snow and ice behave by light as the game's; seeds are refused in the dark;
  the daylight detector works; every mob's read of light sees torches, glass and leaves
  (an undead mob under leaves does not burn, sky 14 there). Natural spawning is N9's.
- Light is made twice for a chunk a player sees: at the edge for the client (ADR-0023)
  and in the region for the rules. They are the same function of the same blocks where
  both have the same 3×3, and a test holds them to it; whether regions later feed the
  edge is a question for after N8.
- A region's chunks start ticking only once their 3×3 is lit: after a restore,
  take-over or move, the rim of a view stands still for about a second (section 6),
  which a player can see in water that waits there.
- A merge or a split costs light only at the faces of the chunks handed back warm;
  chunks the store delivers afresh are lit whole within the budget.
- The pause of a load, a merge, a split and a take-over changes; N8e measures it on the
  real disk and says which disk each number is from.
- `clustine-sim` depends on `clustine-light`, and from N8f on `clustine-features`, which
  adds the worldgen crates to the worker's build.
- Named differences from the game: what the tick changes after the step is lit at the
  next step (section 1); the ranking of dark stroll candidates in a copy or ring chunk
  (section 8); a mob's path cut at a waiting chunk (section 6); a tree's blocks or leaves
  refused across a line, a bee nest without bees, and a stem left turned (section 9);
  the crate's own difference under something floating (section 2). In a world played
  before N8 a farm in a dark room breaks as its neighbours change, as the game's would.

## Steps

As the plan's section 11: **N8a** the contract (the engine's interface with E1's
bodies, the noting in `Chunk::set`, the region's light, the queue and the budget,
`update_ticking`'s test, `restore_loaded`'s promise, the step, the reads and their
views, section 10's check and its setting, the shared files stubbed), the lead's;
**N8b** the incremental engine and its benches, delegated; **N8c** the rules but
saplings, survival, the attached stem's and the bamboo shoot's shape updates, and the
mushrooms' and grass's `SetIf`s, delegated; **N8d** the daylight detector and the sun's
angle, delegated; **N8e** the kept levels at merges and splits, carried light, the
budget's figures from section 11's measurements, and the comparisons with the official
server of light, the detector and the mobs, the lead's; **N8f** saplings, trees and bone
meal (the view for features and the dependency first, the lead's; then the growers,
delegated) and the comparison of rates; **N8g** tests from the specification by someone
who wrote none of it. Not delegated: the noting in `Chunk::set`, the step's place, the
budget and `update_ticking`'s test, the hand-over of kept levels and their adoption, and
the view for features with its effects: the parts where ordering mistakes hide.

## Changes to earlier records

Each gets a note under its own changes heading that points here, with this text.

- **ADR-0023** (light at the edge): "Since ADR-0029, regions light their chunks too, for
  the rules of the simulation, with an engine of their own (`clustine_light::engine`)
  whose oracle is the same `clustine_light::light` the edge's jobs use. The edge is
  unchanged: it keeps lighting what clients are sent, by this record. Nothing about light
  passes between a region and an edge: a region sends no light and an edge's light
  reaches no region; `Chunk::light` keeps its meaning, carried light dropped at the first
  change light can see. Where both have the same blocks in a chunk's 3×3 they hold the
  same light, and a test holds them to it."
- **ADR-0024 section 4** (the restore tests): "`Region::restore_loaded` goes on as the
  region whose state and chunks were taken in the tick before only where the region's
  chunks fit in one light step's budget (ADR-0029, section 6), which every test that
  relies on it does, since a test's region has no budget. Otherwise its chunks wait for
  their light as arrivals do, nearest the players first, and a chunk ticks only once its
  3×3 is lit."
- **ADR-0024 section 8** (light): "Light is now in the simulation (ADR-0029): ice and snow
  melt, crops, stems, berries, bamboo and saplings grow, grass and mycelium spread and
  die, mushrooms spread, and mobs read the region's light. Natural spawning is N9's."
- **ADR-0025 section 4** (effects out), the last row of its table: "Also a grass block or
  mycelium spreading into another region's land, a mushroom's spread, a stem's fruit,
  the blocks of a tree or of a bone meal feature placed through the region's view for
  features (and the leaves whose distance a tree rewrites), each a `SetIf` with the state
  the rule read there as expected (ADR-0029, section 9). A `SetIf` carries no block
  entity: a bee nest grown across a line comes without its bees."

## Built so far

N8a, the lead's contract: the engine's interface with its first implementation, which
lights whole every chunk a change can alter (`crates/clustine-light/src/engine.rs`); the
noting in `Chunk::set`; the region's light step before `update_ticking`, its budget of
arrivals (`LightSettings`: 256 units for a server, none for the tests) and the rule that a
chunk ticks only once its 3×3 is lit (`crates/clustine-sim/src/region/light.rs`); the
reads of the rules' views, the placement's and `MobWorld::light` on it; `restore_loaded`'s
amended promise; the stubs of the shared files for D2 and D3. A chunk let go and loaded
again between two steps is taken as a replacement and lit outside the budget: harmless,
and rare. A region assumes a sky; the dimensions without one come with phase D. The light
handed on at a merge or a split (N8e's first part): `Region::hand_over_light` lights the
last tick's changes and gives each loaded chunk's levels, the runner keeps them beside
the chunk in `Warm` and hands them back in `TickInputs::chunks_kept`, and the region adopts
them at one unit a chunk; a split's part's chunks are lit anew where they go (their levels
do not travel with `Part` yet). Not built yet: the incremental engine (D1), which makes an
adoption cheap (the first implementation lights an adopted chunk whole, which is exact),
the rules (D2), the detector (D3), carried light and the oracle setting (N8e), saplings
(N8f).
