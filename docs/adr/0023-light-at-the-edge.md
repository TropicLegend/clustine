# ADR-0023: Light at the edge

- Status: **Accepted** on 2026-10-11, after two independent reviews whose findings
  are worked in (see "Review"). The second review's verdict was "accept after the
  listed changes; no third review needed". **Built** in steps L0a to L7 of section
  12, on 2026-10-13 and 2026-10-14, which is step W3 of the terrain plan; how they
  were built, what the tests written from the record found and what L7 measured is
  under "Found while building". **L8 is not built**; how far L5 already took what it
  was for is in that section too. The choices it decides for the owner are
  listed under "Choices the owner has not settled"; each can be turned. It is the record that
  [ADR-0022](0022-what-a-chunk-carries.md) hands over in its section 7. It decides
  where the light of a chunk lives in an edge's replica, when a chunk may be sent
  and with what light, what makes an edge compute light anew and where that work
  runs, what a client is sent, and what the versions carried with a chunk mean to
  an edge. It changes `services/edge` (`fanout.rs`, `encode.rs`, new `light.rs`,
  `script.rs` and `light_orders.rs`, `lib.rs`, `Cargo.toml`),
  `crates/clustine-light` (three additions), `crates/clustine-world` (three
  additions), `crates/clustine-format` (one case of one byte),
  `services/worldgen` (one line), `crates/clustine-protocol` (one packet),
  `tools/botswarm` (it takes that packet), the workspace's `Cargo.toml` (three
  profile lines), and it changes ADR-0022 and ADR-0021 in the places "Changes to
  earlier records" lists with their text. It rests on ADR-0022 for what a chunk
  carries, on [ADR-0021](0021-generation-is-a-function.md) for what "as generated
  by version *n*" means, and on ADR-0005, ADR-0012 (section 5), ADR-0013 and
  ADR-0015 for what an edge is told and when.
- Date: 2026-10-11

Where a figure or a statement is a **guess** it says so. Statements about the code
are of the tree at `96d8ecb`. In that tree ADR-0022's steps C1 to C5 are built: a
chunk carries its light (`crates/clustine-world/src/chunk.rs:176-193`),
`clustine_light::light` takes the nine chunks themselves
(`crates/clustine-light/src/lib.rs:69-89`) and the edge's packet is made from what
the chunk carries (`services/edge/src/encode.rs:25-64`). ADR-0020 is built too, and
has changed `fanout.rs` since the second draft was written. Every line cited here
was read again in that tree. Section 10 says, for each thing the second draft
asked of ADR-0022's step C2, whether it was built as asked.

**The number.** The terrain plan's step B1 and ADR-0022 (section 2, on ticks and
marks) call the record for neighbour updates and scheduled ticks "ADR-0023". This
record takes the number because it is written first; that one becomes ADR-0024,
and the three records the replicas plan names move up by one. "Changes to earlier
records" lists every mention.

**Nothing of light at the edge waits for a disk.** The nodes Clustine is to run
on have network storage with about 22 ms for a synced write (the owner,
2026-10-10). An edge reads and writes no file for light: a job reads nine chunks
in memory, its result lives in the replica and dies with the entry, and nothing
an edge computes is stored. Sections 5, 7 and 9 say so again where a disk could
have come in.

## Context

### What the code does today

| What | Where |
|---|---|
| One task owns the replica and every player's view; connections reach it through commands and are sent encoded packets through a bounded queue of 2,048; a player whose queue is full is dropped | `services/edge/src/fanout.rs:1-12`, `363-413`, `2528-2546`; `services/edge/src/play.rs:31`, `42` |
| Its loop takes one thing a turn: a command, a message of a region, a new link or a list of merged regions, or the tick of a one-second interval. Every input of every player is a turn | `fanout.rs:477-511` |
| Going through everything at the end of a turn has cost before: the check of subscriptions runs ten times a second at most outside the edge's own tests, for that reason | `fanout.rs:383-390`, `512-517` |
| The edge runs on tokio's threads for everything; it has no thread of its own. Dropping `Edge::run`'s future stops all of it | `services/edge/src/lib.rs:281-316`; `bin/clustine/src/lib.rs:333` |
| A replica entry is the viewers' count, the chunk if it has come, the kept packet, and who serves it | `fanout.rs:133-145` |
| An entry is made, without a chunk, when a view first wants the position; it is removed when its last viewer moves away or is removed | `fanout.rs:2343-2358`; `2337-2340`, `2892-2901` |
| So an entry exists exactly for the positions some player wants, and "wanted and not yet come" is an entry without a chunk; a chunk that is there when a view comes to want it is pending for that view at once | `fanout.rs:194-200`, `2355-2357` |
| A chunk comes one a message, from whichever region serves it, and is taken only if the answer is this subscription's | `fanout.rs:1043-1053`, `2692-2707`; ADR-0012, rules 8 and 11 |
| The answers of one tick come in ascending order of the chunks, not nearest first, and the messages of all regions pass through one queue of 1,024 | ADR-0012, section 5.2, item 7; `fanout.rs:56`, `438-440` |
| A snapshot that does not show the same as the replica's chunk (`Chunk::shows_the_same`) replaces it, drops the packet, takes the chunk out of what each viewer was sent and offers each a batch at once, **inside** `take_snapshot` and before the snapshot's entities. A client that has the chunk is sent it again without an unload | `fanout.rs:2409-2432`, `2434-2452`; `crates/clustine-world/src/chunk.rs:493-497` |
| **A snapshot that shows the same also replaces the replica's chunk, light and all, and keeps the packet** that was made from the earlier one | `fanout.rs:2413-2414` |
| A block change is applied to the replica's chunk, drops the packet, and goes as a block update to those who have the chunk in `sent`; a change for a chunk that has not come is passed over. It is applied whichever region says it | `fanout.rs:2456-2487`, `2013-2015` |
| `Chunk::set` drops the chunk's light if light could tell the old and the new state apart, by six fields of the block table; whether a state is air is not among them | `chunk.rs:148-162`, `287-289` |
| A batch is the pending chunks nearest the centre, at most `batch_size`, found by sorting all that is pending; every pending chunk is taken to be sendable | `fanout.rs:2491-2524` |
| A batch is offered when a view moves (two callers: `move_view`, and `enter_by_arrival` after the arrival and the login), when a snapshot is taken, and when the client confirms the last one | `fanout.rs:2299-2305`, `2181-2185`, `2429-2431`, `976-992` |
| Who views a chunk, and who has it, is found by going through all players | `fanout.rs:2418-2428`, `2478-2483` |
| **Removing a player is entered from every send**: `send_to_player` removes the player whose queue is full, and returns false. `remove_player` drops the view, counts the viewer out of every entry and removes the entries nobody views any more. It reaches the other players through `try_send` alone, so it never enters itself | `fanout.rs:2536-2543`; `2847-2920`, `2864-2869`, `2886-2890` |
| Entities are shown by what a player wants, not by what they were sent | `fanout.rs:2975-2979` |
| The packet is made on the fan-out task when first needed and kept until the chunk changes | `fanout.rs:147-156` |
| Its light is the chunk's own where the chunk carries any, with `Full` written as an array of fifteens; otherwise `sky_light` over the chunk's heights (full above the highest block, none below) with block light dark wherever sky light is listed | `services/edge/src/encode.rs:87-135`, `137-159`; `crates/clustine-world/src/light.rs:108-145` |
| The packet's blocks are still written through a list of 4,096 values a section and a palette built anew; copying the section's palette and words, which ADR-0022's section 5 decides, **is owed** | `encode.rs:66-85`; the message of `7e8ac03` |
| Each connection compresses what it is sent, on its own task | `services/edge/src/connection.rs:110-119`; `play.rs:183-188`; `lib.rs:56-57` |
| Nothing about light is sent at a block change. The codec has the light update's id and no packet for it; a bot passes over a packet the codec does not model | `crates/clustine-protocol/src/generated/packet_ids.rs:198`; `tools/botswarm/src/bot.rs:1159` |
| Who serves a chunk changes with links, merges and splits; the replica's chunk stays through all of it | `fanout.rs:141-144`, `1661-1665`, `2628-2632`, `2731-2733`, `2772-2775` |
| `region_patience` is how long a player waits for a region, 20 s by default | `lib.rs:58-62`, `81`; `fanout.rs:128-129`, `846-847` |
| The fan-out's settings are four fields, set in one place for the server and once in each of the two test harnesses | `fanout.rs:122-130`; `lib.rs:252-257`; `fanout.rs:3251-3256`, `5343-5357` |
| The edge has 201 tests in `fanout.rs`, in two modules: `tests` (34) and `scenarios` (167). Each scripts the regions itself, private to the file, and sends snapshots of chunks without light and not as generated, one by one; ten run on a paused clock. `scenarios` has a model of the edge that random runs are held to | `fanout.rs:3193`, `5176`; `4133-4145`, `6098-6135`; `8606-8630`, `10080-10095` |
| Those tests know that the edge has sent all it will by counting turns: `settle` waits for the confirmation of an entry, `sync` for the acknowledgement of an action, `shows` is both | `fanout.rs:3328-3343`, `5483`, `5622-5651`, `5654-5658` |
| Their client tells what a chunk holds by comparing the packet's **bytes** with `chunk_packet` of a chunk it builds; it takes an unload and a block update already | `fanout.rs:5911-5919`, `5921-5929` |
| The harness changes air to stone, which light sees | `fanout.rs:6146-6154`, `6288`, `6646` |
| The workspace has no `[profile]`: tests build without optimisation, and a panic unwinds | `Cargo.toml` |
| The lock file is an input of the generated data's sums, and a test fails when it has changed and `cargo datagen` was not run again | `tools/datagen/src/sums.rs:1-21` |

### What `clustine-light` is

| What | Where |
|---|---|
| `light(&Around) -> Light`: sky and block light of the centre chunk, made by `VERSION`, one case for each of the world's sections and one below and above. `Around` is the centre chunk, its eight neighbours from the north-west row by row, and whether the dimension has a sky | `crates/clustine-light/src/lib.rs:59`, `64-89` |
| It reads chunks of `clustine-world` and the block table of `clustine-data`. **The trait `Blocks` of the second draft is gone**; nothing in the crate is generic over what it reads | `crates/clustine-light/Cargo.toml`; `src/flood.rs:100`, `410-423` |
| A chunk that is not there is a wall: it gives no light and takes none | `lib.rs:19-20`; `flood.rs:37-38`, `107`, `117-120` |
| What it reads of a state: its dampening, what it gives, whether it has faces that stop light, and those faces; and of a section its count of what is not air | `flood.rs:57-73`, `133`; `src/faces.rs:18-31` |
| A section is filled as one state where it is uniform, or where every state it may hold is the same to light and has no faces of its own (stone with its ores); otherwise it is read block by block through `Section::states` | `flood.rs:134-167`; `crates/clustine-world/src/section.rs:352-354` |
| A section is `Absent` unless it or one of the 26 around it, over the nine chunks, holds a block that is not air; the levels of such a section are reckoned and then left out. Whether a section is `Absent` looks one section up and down; the sky's light in it depends on a roof any height above | `flood.rs:347-375`, `259-315`; `lib.rs:29-35` |
| "The game … reads a section without any [light of its own] from the one above it"; under something that floats the crate and the game may differ, and no chunk of the sample shows which | `lib.rs:37-45` |
| Every call makes its grid anew: 1,045,000 cells for a world of 24 sections. Kinds 1.0 MB, states 2.1 MB, and two arrays of levels of 1.0 MB one after the other: **4.2 MB allocated a call, 4.2 MB at the peak** (the sky's levels are dropped before the block light's are made) | `flood.rs:22-26`, `104-108`, `246`, `276`, `412-417` |
| There is no relight from a change | the whole crate |
| The time of a call is taken by the comparison with the official server's stored light and printed; **the figures are written nowhere in the tree**. The main session reports from step C4: about **2.2 ms** a chunk of the sample in a release build and about **15 ms** in a build without optimisation. Step L0 writes its own figures into this record; section 9 is 2.2 ms times a count | `crates/clustine-light/tests/official.rs:374-421` |

### What ADR-0022 fixed, and handed over

Light is carried with the version of the rules that made it and is a thing apart
from "as generated"; a chunk drops its light at the first change that light sees;
the edge compares what is shown and not light; until this record is built the
edge sends a chunk's own light where it has any and `sky_light` otherwise, and
nothing about light at a block change. All of that is in the tree. It handed over
eight items, which section 11 answers one by one.

## Decision

**An edge keeps one light for each chunk of its replica, beside the chunk: the
light the chunk came with while a test on the chunk and its neighbours passes,
and otherwise light the edge computed. A chunk is sent only with light. Whatever
light can see that changes in a chunk's three by three marks the chunk; marked
chunks are worked off on threads of the edge's own, chunks someone waits for
first, one job a chunk, from nine shared chunks that no thread changes. What a
job gives is sent as the chunk to those who wait for it and as a light update to
those who have it, unless a block changed under the job: then those who have the
chunk get the next job's.**

### 1. Where light lives

A replica entry (`ReplicaChunk`, `fanout.rs:133-145`) becomes:

```rust
struct ReplicaChunk {
    viewers: u32,
    /// The chunk without light: the edge takes the light out when the chunk comes.
    chunk: Option<Arc<Chunk>>,
    /// Kept only while the entry has a waiter (below).
    packet: Option<Bytes>,
    served_by: Option<RegionId>,
    /// The light a client is sent with the chunk. One copy.
    light: Option<Arc<Light>>,
    kind: LightKind,                 // Carried, Computed, Fallback
    /// The generator's version whose blocks, as light sees them, the chunk has.
    as_made: Option<u16>,
    shaped: u64,                     // moments, below
    changed: u64,
    lit: u64,
    /// The moment of the job under way for this chunk, if any.
    job: Option<u64>,
    /// The players who have the chunk pending.
    waiters: BTreeSet<PlayerId>,
    /// Those who have the chunk were not sent the last light: the next goes whole.
    owed: bool,
    /// When the entry last stopped being sendable; for the hold.
    unsendable_since: Option<Instant>,
    /// When the entry last stopped being current; orders those nobody waits for.
    marked_since: Option<Instant>,
    /// The first block change that reached the chunk since its job was dispatched.
    retouched: Option<Instant>,
    dispatched: Option<Instant>,
    /// Which of the task's sets the entry is in, with its key there (section 4).
    queued: Option<Queued>,
}
```

Section 12 says which step adds which field: none is added before the step that
reads it.

- **One copy of the light.** The light a chunk came with is moved out of the
  chunk into the `Arc` when the chunk comes (`Chunk::take_light`, section 10);
  computed light takes its place in the same field. `Light` is `clustine-world`'s
  (`crates/clustine-world/src/light.rs:95-100`), which is also what
  `clustine_light::light` returns.
- **The packet is kept only while the entry has a waiter.** It is made when the
  first batch that carries the chunk is made, handed to every further waiter as
  the same bytes, and dropped when the last waiter has been sent the chunk or has
  gone; and, as today, at every change. A packet is 32.5 KB at the mean for real
  terrain (`0022:546-548`), as much as the chunk and its light together, and for
  players who are apart it is never read a second time: section 9 has the sums.
  What it costs is that a player who arrives where others already are has every
  packet made again, once a chunk, on the fan-out task; L7 measures that turn.
  The packet has the light's bytes inside it, about 12 KB with full sections
  written as arrays (`0022:324`), for as long as it lives.
- **The chunk is in an `Arc`**, so that a job can be handed nine chunks by nine
  pointer copies. A block change goes through `Arc::make_mut`, which copies the
  chunk (about 23 KB at the mean without its light: ADR-0022's 32.0 less 9.3)
  only while a job holds it. A change that puts the state that is there already
  is found by `get` first and copies nothing.

**Moments.** The fan-out task has one counter for the whole edge, `moment: u64`,
from 1, raised by one every time it is read. It is never stored and never comes
again while the edge runs, whatever is removed from the replica and made anew
(ADR-0022's item 5). An entry has three:

| Moment | Is raised | Means |
|---|---|---|
| `shaped` | when the entry's chunk comes or is replaced, and when a neighbour's does | which chunks light is made from has changed |
| `changed` | at a block change that light sees, in the chunk or in a neighbour it reaches (section 3) | a block light sees has changed |
| `lit` | when light is put into `light`: the moment of the job that made it or failed, or of the test that passed | what `light` is as recent as |

An entry with a chunk is **sendable** if it has light and `lit >= shaped`;
**current** if it is sendable and `lit >= changed`; **marked** if it is not
current. A marked entry that is sendable is **stale**: by block changes alone.

### 2. The standing rule

**L. At the end of every turn of the fan-out task, for every player X and every
chunk `p` in `X.sent`:**

- **L1.** X's client has been sent, for `p`, exactly `entry(p).light`: in the
  chunk's packet, or in light updates since. **Or** `entry(p).owed` is set, and X
  has a light the entry had before; then the entry is marked or is `Fallback`,
  and the next result that is taken and not withheld (section 5) is sent to X
  with every section named.
- **L2.** Either `entry(p)` is current, or it is marked and in one of the task's
  three sets or has a job under way.
- **L3.** No chunk was ever sent from an entry that was not sendable at that
  moment. In particular none without light.
- **L4.** For a chunk in `X.pending` whose entry has a chunk and is not sendable:
  its job is dispatched **before any job for a chunk nobody waits for**; and it is
  dispatched as soon as it is not held (at most `LATEST` after it became
  unsendable) and a slot is free, behind only those chunks someone waits for that
  are **nearer to their nearest waiter as the views stand now**, and, among the
  equally near, those that were put in before it (section 4). It needs one job
  for each time a chunk came or was replaced in its three by three while a job
  for it was under way, and one more. A job ends in a result, fails at its
  dispatch because no thread is left, or is given up `JOB_PATIENCE` after a
  thread took it. So with nothing further from any region, no view moving and
  every job answered, the chunk is sent; section 9 has the time. **What L4 does
  not promise**: that a far chunk is ever dispatched while nearer chunks keep
  coming faster than the threads light them (section 4).

**What "current" promises of the light itself**, by `kind`:

| Kind | The light is |
|---|---|
| Carried | the light the chunk came with, and the test T (section 7) passed when the entry was last shaped, and no block that light sees has changed in its three by three since |
| Computed | `clustine_light::light` over the chunk's blocks as light sees them now and the neighbours that were in the replica when its job was dispatched, none of which has come anew, been replaced or been changed within reach since; **with every section that the entry's light once listed still listed, as its levels** (section 6) |
| **Fallback** | **not right, and counted**: the last job for the chunk failed or was given up. It is the light the entry had before that job, made for other blocks or beside other neighbours, or, if it had none, `sky_light` over the chunk's heights with block light dark. It stays until something marks the chunk again; no job is made for it by itself (section 5) |

A neighbour that has **left** the replica since is not in that table: section 3
says why leaving marks nothing.

### 3. What marks a chunk

Six events change the replica or a view. They are the only places light is
looked at.

| # | Event | Where today | What it does to light |
|---|---|---|---|
| E1 | A chunk comes into an entry that had none | `fanout.rs:2392-2432` | **arrive** (below) |
| E2 | A snapshot comes for an entry whose chunk shows the same (`Chunk::shows_the_same`, `chunk.rs:493-497`) | `fanout.rs:2409-2414` | nothing: light follows from blocks, and the blocks are the same. **The snapshot's chunk is dropped, its light with it, and the entry keeps the chunk it has**, where today's code puts the new one in. A job under way for the chunk, or for a neighbour, goes on and its result is taken. It is the same whichever region the snapshot is from (after a merge, a split or `NotMine`) |
| E3 | A snapshot comes that shows otherwise (a resume after missed changes) | `fanout.rs:2415-2432` | **arrive**; every viewer is to be sent the chunk again, **after an unload** (section 6) |
| E4 | A block change in which light cannot tell the old and the new state apart | `fanout.rs:2456-2487` | the packet is dropped, the block update sent, as today. No moment is raised. **A change that puts the state that is there already is this case too**: two regions can say the same change (`fanout.rs:2013-2015` asks nobody who serves the chunk), and the second raises nothing |
| E5 | A block change that light sees | the same | **change** (below) |
| E6 | An entry is removed: its last viewer moved away (`shift_view`) or was removed (`remove_player`) | `fanout.rs:2337-2340`, `2892-2901` | it is taken out of the task's sets and its packet goes with it. A result for it is dropped when it comes (section 5). `remove_player` does this for many entries in one go, and from inside a send: section 4 has the rule |

Nothing else touches light: not a link that ends or comes, a welcome, `Elsewhere`,
`NotMine`, a merge, a split, a hand-over. All of them change who serves a chunk
and none changes the chunk the replica has (`fanout.rs:1661-1665`, `2628-2632`,
`2731-2733`, `2772-2775`). Nothing here reorders what the edge takes from a link,
so no rule of ADR-0012's section 5 or of ADR-0015 is touched.

**arrive(`p`, chunk):**

1. Take the light out of the chunk. `as_made` is: the light's `beside`, if the
   chunk came with light and `beside` is not 0, **whichever rules made the
   light**; otherwise the chunk's `generated`, if not 0; otherwise none (section
   7). (The second draft asked for an acceptable `made_by` here. By ADR-0022's I1
   nothing light sees has changed since the light was made, whoever made it, so
   the condition cost the neighbours jobs and bought nothing: the second review's
   U8.)
2. Put the chunk in. Drop the packet. **Forget the job under way**, if any
   (`job = None`). `light = None`, `owed = false`. `shaped =` the next moment.
3. If the light taken out is acceptable and T(`p`) passes: it becomes `light`,
   `kind = Carried`, `lit = shaped`. Otherwise `p` is unsendable from now.
4. For each of the eight neighbours `q` that has a chunk: `q.shaped =` the next
   moment. If `q` is `Carried`, was current and has no job under way, and T(`q`)
   passes, `q.lit = q.shaped`. Otherwise `q` is unsendable; **from now, if it was
   sendable**: `unsendable_since` and `marked_since` are not set anew while an
   entry stays unsendable or marked, so a hold ends. **A `Fallback` entry is
   never kept by this step**: a new neighbour is a new input, and it gets a job.
5. Viewers: the chunk is pending for everyone who wants it, and each of them is
   put into `p.waiters`. **If `p` is sendable now, its waiters are offered a batch
   here, inside `take_snapshot` and before the snapshot's entities, as today.**

**change(`p`, position, new state):** if the state is the one that is there,
nothing but the block update (E4). Otherwise the block is set (the chunk copied
first if a job holds it), the packet dropped, and the block update sent to those
who have the chunk in `sent` **or in `replace`** (section 6). If light can tell
the old and the new state apart: `p.as_made = None`; `p.changed =` the next
moment; and the same `changed` for each neighbour with a chunk that the change
**reaches**. Each of them that was current is marked from now
(`marked_since = now`); each that has a job under way and no `retouched` gets
`retouched = now`.

- **Whether light can tell two states apart** is
  `clustine_world::light_tells_apart(old, new)`: today's private function of
  `chunk.rs:148-162`, made public, **with `is_air` added to what it compares**
  (section 10). It is the one function `Chunk::set` asks before it drops carried
  light, in the simulation, the store and the edge alike.
- **Reach.** A level falls by at least one a step, so a change in a column alters
  light at most 14 steps sideways from it. The change reaches the neighbour at
  offset (`dx`, `dz`) if `hx + hz <= 14`, where `hx` is 0 for `dx = 0`, `x + 1`
  for `dx = -1` and `16 - x` for `dx = 1`, and `hz` likewise. That is each of the
  four neighbours beside for 14 columns of 16 along its side, and **each**
  diagonal neighbour for 91 columns of 256 (some diagonal neighbour for 244 of
  256): 5.9 chunks a change at the mean, the chunk itself among them, not 9.
  **And it reaches all eight** if it made the first block of a section that held
  only air, or took the last: which sections are `Absent` turns on that, 26
  sections around (`flood.rs:133`, `347-353`). `clustine_light::reach(x, z)` gives
  the eight answers. (The first review went over the argument against the
  flood's cut by distance, now `flood.rs:112-114`, `225-228` and `378-382`, and
  the second against the rule for `Absent`, and both found it sound; test 1
  checks it on random worlds.)
- **A neighbour leaving marks nothing.** A chunk lit beside a neighbour that has
  since left keeps light that is right for the world, where a relight would put a
  wall in the neighbour's place and make it darker. It would also cost every
  chunk a third job as a view passes over it. A neighbour is absent only if no
  player of this edge wants it, so the chunk beside it is at the rim of every
  view that has it; its return is always E1, which marks. (Both reviews looked
  for an order of events in which wrong light outlasts the neighbour's return
  and found none.) This departs from "a neighbour entering or leaving" in the
  brief for this record; choice 3.
- **A change for a chunk that has not come** is passed over, as today
  (`fanout.rs:2461-2464`): the snapshot is the chunk after its tick (ADR-0012,
  rule 11) and has it.

### 4. The sets, and when a job is dispatched

**Nothing is scanned at the end of a turn.** The fan-out task keeps:

| What | Is | Changed by |
|---|---|---|
| `waiting` | the entries that are due, are not sendable and that someone waits for, ordered by (rank, the moment the entry was put in) | below |
| `unawaited` | the other marked entries that are due: **the stale, and the unsendable that nobody waits for** (a neighbour came beside a chunk everyone has). Ordered by (rank, the moment put in); and the same entries ordered by `marked_since` | below |
| `later` | the marked entries that are held or resting, ordered by the time at which that ends | below |
| `entry.waiters` | the players who have the chunk pending | wherever `pending` is changed: `shift_view` (`fanout.rs:2326`, `2356`), `take_snapshot` (`2425`), `send_chunks` (`2511`); **and `remove_player`, which drops a whole view (`2848`)** |
| `view.ready` | how many of the player's pending chunks are sendable | when an entry becomes sendable or stops being so, for each of its waiters; and with `pending` |
| `view.replace` | chunks the client still has from before E3 | E3, the sending, a view that moves; it goes with the view |

(The second draft called the second set `stale`, which it was not: the second
review's U3. Its age is `marked_since`, which every entry in it has: it is set
when an entry stops being current, by a change or by a neighbour's coming, and
not set anew by a block change in an entry that is already marked.)

- An entry is in at most one of the three sets, and `entry.queued` says which and
  under what key, so that it is taken out or moved in a logarithm's time.
- **Rank** in `waiting` is the squared distance from the chunk to the centre of
  its nearest waiter. **It is kept up when a view's centre moves**: `shift_view`
  goes through the mover's pending chunks that are in `waiting` and moves each
  whose rank the move changes, keeping the moment it was put in. The entry
  remembers which waiter it is ranked by; its rank is found anew over all its
  waiters only if that waiter is the one who moved away from it, and otherwise
  it is the lesser of what it was and the mover's new distance. **The cost** is
  the mover's pending chunks that are not sendable, once a chunk border crossed:
  a logarithm each and, for a chunk whose nearest waiter moved, a walk over its
  waiters. That is nothing in ordinary play (a walking player has a row of new
  chunks pending for a moment) and at most a view, 329 entries, for a player
  flying under overload, about 1.4 times a second.
- **Among equal ranks, first come, first served**: the second part of the key is
  the moment (the edge's counter) at which the entry was put into `waiting`,
  which a move within the set keeps. The second draft broke ties by position,
  which served low coordinates first and put every newly arrived rim chunk of a
  player flying west or north before the older ones in front of them (the second
  review's finding 3).
- **Rank in `unawaited`** is the squared distance to the nearest viewer's centre
  when the entry was put in, and **is not kept up**. Its entries are shown with
  light that is a little old; which of them goes first matters less than that
  the oldest do, which `OVERDUE` sees to.
- **A turn costs what it touched**: the entries of the events it handled and
  their neighbours, each in a logarithm's time; a dispatch a slot; an offer the
  waiters of the chunks that became sendable. Not the replica. **Two things
  still walk the players**, as they do today, and are cheap: those who have a
  chunk are found by asking every view (`fanout.rs:2478-2483`), a hundred
  look-ups for a block update or a light update with a hundred players, since
  the entry has an index of its waiters and none of its holders; and an offer
  sorts what its player has pending to find the nearest sendable
  (`fanout.rs:2499-2506`), a hundred sorts of up to a view for a result that a
  hundred wait for. With `ready` an offer that would find nothing is not made.

An unsendable entry with a chunk is **held** while some neighbour's entry has no
chunk yet and `unsendable_since` is less than `LATEST` ago. A stale entry
**rests** until `REST` after its last dispatch, unless it is exempt (section 5).
A marked entry with no job under way is in `later` while held or resting, with
the time that ends it; otherwise in `waiting` if it is unsendable and has a
waiter; otherwise in `unawaited`.

**The hold and the rest are settings of the fan-out, not constants.**
`FanoutConfig` (`fanout.rs:122-130`) gets `light: LightSettings { latest, rest,
overdue, job_patience }`. The server sets `LATEST` 1 s, `REST` 250 ms, `OVERDUE`
5 s and `JOB_PATIENCE` 5 s (`lib.rs:252-257`). **Both existing harnesses set
`latest` and `rest` to nothing** (`fanout.rs:3251-3256`, `5352-5357`), so that an
entry is never held and never rests there: with the end of `Lighting` that runs
in the dispatching turn (section 5), everything a turn marks is lit before the
turn ends, and `settle` and `sync` go on meaning that the edge has sent all it
will. The hold and the rest are tested by test 4 alone, which sets them itself
on a paused clock. (The second review's finding 1: with the constants, the
harness's first air-to-stone change would have left every later arrival beside
it held for a second of the clock, and the old tests and the random runs red.)

**Where entries are moved**: in arrive and change, for the entries they touch;
when a neighbour's chunk comes, which can end a hold, for the eight around it;
when a waiter is added or leaves, **which only moves entries between sets and
never sends or dispatches** (`enter_by_arrival` shifts the view, then sends the
arrival and the login, and offers chunks last, `fanout.rs:2175-2185`: that order
stays); when a view's centre moves; when a result is taken; and at a tick of
50 ms, which runs only while `later` is not empty or a job is under way and
moves the entries whose time has come. With `latest` and `rest` of nothing and
jobs done in the turn, that tick never runs: the ten tests on a paused clock
see no new timer.

**When a player is removed.** `remove_player` (`fanout.rs:2847-2920`) is entered
from inside every `send_to_player` whose queue is full (`2536-2543`): in the
middle of the fan-out of a result, of `take_snapshot`'s offers, of
`tell_entered` (`2256-2258`, `2291`). Three rules keep the sets right through it:

- **R1. What `remove_player` does to light.** After it has taken the view out of
  `players`: for each chunk in the view's `pending`, the player leaves that
  entry's `waiters`; an entry in `waiting` that has no waiter left goes to
  `unawaited`, and one that has is ranked anew if the player was the waiter it
  was ranked by. `ready` and `replace` go with the view. Then, as today, the
  viewer is counted out of every entry of `wanted`, and each entry nobody views
  any more is E6: out of its set, its packet dropped, its job left to answer to
  nobody. An entry that keeps viewers and has lost its last waiter drops its
  packet. All of it is done before `remove_player` awaits anything, and it sends
  to no player but through `try_send`, as today.
- **R2. Everything first, then the packets.** An operation that sends to players
  (taking a result, arrive's offers, a batch) finishes what it changes in the
  entry, in `waiters`, `ready`, `replace`, `sent` and the sets before its first
  `send_to_player`. It collects those it will send to, by id, before it sends
  to any (as today's code does, `fanout.rs:2418-2431`, `2478-2486`), and the
  packets it will send them are encoded by then.
- **R3. After a send, nothing is taken for granted.** A send that returns false
  has removed that player, and with them possibly entries: the entry the
  operation is about, if they were its last viewer, and others, some with a job
  under way. So after any `send_to_player` the operation looks the entry and the
  view up again before it reads either, and an entry or a view that is gone is
  passed over, not an `expect`. A send to one player never removes another.

The debug check at the end of a turn (`fanout.rs:512-517`) holds, besides the
moments and the sets: every member of an entry's `waiters` is a player of
`players` who has the chunk in `pending`, and the other way round; `ready` is
the count of the player's pending chunks that are sendable; `replace` is within
`pending`.

**dispatch** runs when a slot is free (fewer than `IN_FLIGHT` jobs under way)
**and** this turn put an entry into `waiting` or `unawaited`, took or dropped a
result, or was the tick. It runs at the end of the turn, so that all the block
changes of one tick, which come in one `TickDelta` (`fanout.rs:1054-1058`), are
in the replica before a job is made from it. While a slot is free it takes:

1. the first of `waiting`, **always, while there is one**;
2. otherwise the oldest of `unawaited`, if its `marked_since` is more than
   `OVERDUE` ago;
3. otherwise the first of `unawaited`.

For the chosen `p`: `basis =` the next moment; `p.job = Some(basis)`;
`p.dispatched = now`; `p.retouched = None`; the job goes to the threads with the
position, the nine `Option<Arc<Chunk>>` of its three by three as they are now,
whether the dimension has a sky, `p.light` as the old light, and whether its
update is to name every section (section 6). The task remembers
`basis -> (p, now)` for as long as the job is under way. **If no thread is left
(section 5), the job fails here**, in this turn, and is never queued.

- **Waited-for chunks are never behind a chunk nobody waits for.** Each needs a
  bounded number of jobs (L4), and nothing from `unawaited` comes before one.
  `OVERDUE` orders `unawaited` alone, so a place where blocks never stop
  changing takes no job from a player who has nothing to look at. (The first
  draft put the overdue first of all; the first review's finding 1 showed a join
  that is never served.)
- **Among themselves the waited-for go nearest first, and that is all that is
  promised.** With rank kept up and ties by age, a player's holes fill from
  where they stand outwards wherever they fly, and no coordinate is served
  before another. **Under lasting overload the far chunks of one player wait
  behind the near chunks of all**: while players keep flying into terrain that
  needs jobs faster than the threads do them (two threads do about 900 a second
  at 2.2 ms; one flying player asks for about 56, section 9), a chunk at the rim
  of a standing player's view can go undispatched for as long as that lasts. The
  nine around every player are always before anyone's rim. The second draft said
  that the waited-for "cannot starve each other"; that sentence is struck. An
  age after which a waited-for chunk goes first whatever its distance would
  bound it, at the cost of holes near a flying player; it is not built, and
  choice 12 names it.
- **Those nobody waits for can be starved by the waited-for**: while players
  keep arriving or flying through changed terrain faster than the threads light
  it, light updates for what others already see wait. That is the order wanted;
  section 9 names it.
- **Held** is what makes a join cost one job a chunk. A chunk that comes before
  its neighbours waits for those some player wants; those nobody wants have no
  entry and are not waited for (ADR-0022's item 3, with "wanted by anyone" for
  "wanted by a player who waits", which needs no second index and holds a chunk
  for at most `LATEST` longer). `LATEST` is the patience for a neighbour that does
  not come, in place of `region_patience`, which is 20 s and about something
  else: a chunk the store cannot read is answered with nothing (ADR-0012, rule 9)
  and must not hold its eight neighbours for 20 s. A neighbour that comes after
  `LATEST` costs one more job and one light update; section 9 says when that is
  the usual case.
- **A stale chunk is never held**: a place where water flows is never still
  (ADR-0022's item 1).
- **Rest** bounds the jobs of one chunk to four a second, and lets the first
  change after a quiet time through at once.
- **A chunk with carried light that is current is sent at once, as today**,
  neighbours or none. If a neighbour then comes changed, the chunk is unsendable,
  held until the rest of its neighbours are in, lit once and corrected by one
  light update.

### 5. Where the work runs, and what becomes of a result

**Threads of the edge's own**, `EdgeConfig::light_threads`, by default a quarter
of the processors and **at least two**. They take jobs from one queue and put
what they have to say into a channel the fan-out task reads as one more arm of
its loop (`fanout.rs:477-511`). `clustine_light::light` is never called on the
fan-out task, nor on any tokio thread.

- **A job never reads the replica, and no disk.** Its input is nine pointers to
  chunks nobody changes: 72 bytes copied, about 200 KB of blocks shared.
- **A job's work** (`relight`, pure, in `services/edge/src/light.rs`): light of
  the centre by `clustine_light::light_with` over an `Around` of the nine
  chunks, with a **scratch the thread keeps** in place of 4.2 MB allocated a
  call; the sections once listed kept listed (section 6); and **one** encoded
  light update: the sections in which the result differs from the old light, or
  every listed section if the job was told so at its dispatch. (The second draft
  had every result carry both. Which is wanted is known when the job is
  dispatched: `owed` is set only where a result is withheld, and a chunk has one
  job at a time. The second review's N6.)
- **A thread says which job it has.** Before it starts a job it sends
  `Started { basis, thread }`, and the task notes it. `JOB_PATIENCE` runs from
  that word, not from the dispatch, so a job that waits in the queue behind
  others is never given up, and the task knows which thread to write off for
  which job (the second review's U6).
- **The scratch is made at a thread's first job**, not when the thread starts:
  the end-to-end tests start many edges in a process, most of which never light
  a chunk. It is made anew after a job that panicked, which may have left it
  half written.
- **The threads end** when the fan-out task is dropped: the queue of jobs closes
  and each thread leaves its loop after the job it is in. Nobody joins them, so
  dropping `Edge::run`'s future does not wait (`lib.rs:281-282`).
- **The threads are behind a channel pair**, and three things can be at its
  other end: the threads; **the dispatching turn itself**, for the edge's
  existing tests: `dispatch` runs `relight` where it stands and takes the result
  before the turn ends, so that `settle` and `sync` still mean that the edge has
  sent all it will (section 4, on the settings; section 12, L4); and the test
  itself, which holds jobs and answers them when and how it likes (test 4). (The
  second draft had a fourth, a task that answers at once. No step uses it, and
  code nothing uses does not pass `clippy`; it is gone.)

**A result is taken if and only if** its `basis` is still remembered, the entry
exists, and `entry.job == Some(basis)`. Then `job = None`, and the result's light
is put in (`light`, `kind = Computed`, `lit = basis`, the packet dropped). What
is sent depends on what happened under the job:

| Under the job | To those who wait | To those who have the chunk | Then |
|---|---|---|---|
| Nothing | the chunk | the job's update. `owed = false` | current |
| **A block change reached it** (`changed > basis`) | the chunk, **as its blocks are now**, with this light | **nothing: withheld.** `owed = true` if anyone has the chunk | stale since `retouched` (`marked_since = retouched`); **the next job is exempt from the rest**, once in a `REST` |
| **A chunk came or was replaced beside it** (`shaped > basis`) | nothing: it is not sendable, and they go on waiting | the job's update. **It can be darker along the new neighbour's border than what they had**: it was made with a wall where that neighbour now is, and may replace carried light that was made beside it. The next job, which is already owed to the entry, puts it right | into `waiting` or `unawaited`; held only while `unsendable_since` allows |
| Both | nothing | nothing: withheld | the two rows above |

**The order in which a result is fanned out** follows R2 and R3 of section 4:
the light is put in, the moments and the sets are brought to what the table
says, those who have the chunk and those who wait for it are collected, and only
then are the updates sent, one player after the other, and the waiters offered a
batch, one after the other. A player whose queue is full at their turn is
removed and the others still get theirs; if that player was the entry's last
viewer the entry is gone, and whoever is next on the list (nobody, then) is
looked up before anything is read.

**Why a result is withheld.** The block update went at once, and the client lit
the change by itself (G1). A result made from the blocks before the change, sent
after it, would put the client's own light out in every section it names: a torch
goes dark for a quarter of a second. So those who have the chunk get the next
job's result, whole, and that job does not wait for `REST`. The session's
decision names a change in the chunk itself; a change in a neighbour within reach
is treated the same, because the client's light crosses the border as ours does
and the entry has one moment for both. Choice 6.

A result that is **not** taken:

| The result is | Order of events | What is done |
|---|---|---|
| For a chunk that was **replaced** | dispatch, E3 at `p`, result | dropped: arrive forgot the job |
| For a chunk that **left** | dispatch, E6, result | dropped: no entry |
| For a chunk that left and **came again** | dispatch, E6, E1 at `p`, result | dropped: the new entry has no job of that moment |
| **Late**: its `basis` was given up | | dropped |

A forgotten job still counts against `IN_FLIGHT` until it answers or is given
up, which is what keeps the queues bounded.

**ADR-0022's item 1(b) is answered otherwise than proposed.** It proposed to
build the packet from the job's own snapshot and send the block updates since
after it. Here the packet is made, as today, from the replica's chunk as it is,
with the job's light. The client ends with the same blocks and the same light
either way; this way keeps no list of changes a chunk and adds no second order
of packets to the part of the edge where orders go wrong. It is the right choice
only while the packet is made on the fan-out task; section 11, item 7.

**A failed job** is caught on its thread (`catch_unwind` around `relight`; the
workspace does not abort on a panic), counted, and logged at the level of an
error at most once a second with the count since. Then, whatever the entry held:

- `job = None`, `lit = basis`, and **`kind = Fallback`, always**. The entry keeps
  the light it has; if it has none it is given `sky_light` over its heights
  (`crates/clustine-world/src/light.rs:108-145`) with block light dark wherever
  sky light is listed, as `encode.rs:123-124` does today. (The second draft left
  the kind as it was when the entry had light. A `Carried` entry in which a
  torch was placed and whose job then failed would have been "current, carried"
  with the light of before the torch, and arrive's step 4 would have kept it
  through the next neighbour: the second review's finding 5.)
- **Those who wait are sent the chunk**, with that light. A chunk that is never
  sent is worse than one lit as all chunks were lit before this record.
- **Those who have the chunk are sent nothing**, and **`owed` is set if anyone
  has it** (it stays set if it was). What they hold is the light they were sent
  and what their own client has made of the changes since, which is better than
  light from before those changes sent over it.
- **No job is made for it again until something marks it**: a block that makes
  the rules panic must not make three hundred jobs a second. The next change or
  neighbour marks it like any other entry, and the result of that job goes to
  those who have the chunk with every section named.
- The edge publishes how many jobs have failed and how many entries are
  `Fallback` now, beside its count of players (`lib.rs:232-233`).

**A job that is not answered `JOB_PATIENCE` after a thread took it** is given up
as failed, and **its thread is written off: a new one is started in its place.**
The old one cannot be stopped; if it ever answers, the result is late and
dropped. Each thread written off is an error line. After four the edge starts
no more and goes on with the threads it has. (The flood has no loop that looks
able not to end; this is for the thread that died some other way.)

**With no thread left, a job fails at its dispatch**, in the dispatching turn,
with a line that says so once a minute: every chunk gets the fallback at once
and nothing waits `JOB_PATIENCE`. **What a join waits at worst** follows: if
every job hangs, each of `T` threads is written off after `JOB_PATIENCE`, four
are replaced, and after `4 + T` jobs none is left: with two threads that is
three rounds of 5 s, **15 s**, after which the remaining chunks of a join of 329
are sent at once with fallback light. (The second draft gave up every job after
5 s also when no thread was left: about seven minutes for a join, which the
first review's finding 10 had named.)

**The queues are bounded by construction**: at most `IN_FLIGHT` jobs exist (twice
the number of threads), so neither the queue of jobs nor that of results holds
more.

### 6. What a client is sent, and when

| What | To whom | When | With |
|---|---|---|---|
| The chunk | a player who has it pending | in a batch, when the entry is sendable | `entry.light` |
| An unload, then the chunk | a player who has it in `replace` | the unload goes immediately before the batch that carries the chunk | the same |
| A light update | every player who has the chunk in `sent` (not one who has it in `replace`: their client holds the chunk of before, and is about to be sent the new one whole) | when a result is taken and not withheld | the sections that differ, or all (section 8) |
| Nothing | | a result in which no section differs; a withheld result; a failed job, for those who have the chunk; a player with nothing sendable | |

- **A batch is made of the sendable among the pending** chunks, nearest first, at
  most `batch_size`. It is offered only where `view.ready > 0` and no batch is
  outstanding: to the waiters of a chunk that has just become sendable, when a
  view moves (`move_view`, and `enter_by_arrival` as its last act), and when a
  batch is confirmed. **If none is sendable nothing is sent**: no start, no end,
  and `batch_outstanding` stays false (`fanout.rs:2495-2523` would send an empty
  batch once pending chunks can wait).
- **A light update is never sent for a chunk the client does not have.**
- **The first light update after a block update can be older than the block**
  only if it was made under a job the change did not reach; one that the change
  reached is withheld.

**A chunk sent again is unloaded first.** At E3 a viewer who has the chunk has
it moved from `sent` to `pending` (`fanout.rs:2424-2425`) and, new, into
`replace`. The client goes on showing the old chunk, and block updates for it
still go, until the new one is sendable; then `UnloadChunk` and the chunk go
together. A view that moves away unloads what is in `replace` as it unloads what
is in `sent`: today `shift_view` unloads only what `sent.remove` returns true
for (`fanout.rs:2326-2332`), and a chunk waiting to be replaced is no longer in
`sent`. A second E3 for the same chunk before its light is there leaves it in
`replace`, once. So what the client holds of a chunk is always what one packet
and the updates after it said, whatever the client does with sections a second
packet would not have named (G4), and a block the player broke in such a chunk
does not come back while the chunk waits for its light (the first review's
finding 13).

**The cases of a light section**, for sky and for block light alike:

| Case | In the chunk's packet | In a light update |
|---|---|---|
| `Levels` | its bit in the mask, 2,048 bytes | the same |
| `Full` | its bit in the mask, 2,048 bytes of `0xff` | the same |
| `Dark` | its bit in the empty mask | the same |
| `Absent` | in neither mask | never: there is nothing to say it with |

That is what `encode.rs:137-159` does for a chunk's packet today; L2 makes it a
function of the two lists and of which sections to name, for both packets.

**A section that was once listed stays listed for the life of the entry.** A
light update cannot take a section away (G2), and a section the flood now finds
`Absent` (the top of a tower taken down) still has levels, which the flood
reckons and leaves out (`flood.rs:356-361`) and which go on changing with a roof
any height above (`flood.rs:259-315`). So `relight` asks the flood for the levels
of every section the old light listed and the new would not, and puts them into
its result as `Levels`, `Full` or `Dark`. The entry's light then lists the
section; it is compared and named like any other from then on; a client that
comes later is sent it too, which is right light and at most 2 KB. It is
forgotten when the entry is replaced (arrive) or removed, and by then every
client has unloaded the chunk. The session decided that the entry remembers
which sections each client was sent as levels; this keeps it for the entry, not
for each client, which is the same for every client that had the section and
harmless for one that had not. Choice 10. The masks are one word: 26 bits.

### 7. Versions, and the test for carried light

**T(`p`)**: `p` came with light whose `made_by` is acceptable and whose `beside`
is a version *g* other than 0; and every one of its eight neighbours that has a
chunk in the replica has `as_made == Some(g)`.

- **Acceptable** `made_by`: the edge's own `clustine_light::VERSION`
  (`crates/clustine-light/src/lib.rs:59`), or `Light::IMPORTED`
  (`crates/clustine-world/src/light.rs:105`).
- **`beside`** is new, and not in the tree (section 10): the version of the
  generator that made the nine chunks the light was made from. A chunk that was
  changed in ways light does not see has lost `generated` (`chunk.rs:285`) and
  kept its light (`chunk.rs:287-289`); without `beside` the edge could not tell
  which neighbours that light was made beside, and the light ADR-0022 lets such
  a chunk keep would be of no use to the one service that reads it.
- **`as_made`** says that a chunk's blocks are, as light sees them, what
  generator *g* made. A chunk that came with light has it from `beside`,
  whoever's rules made the light: by ADR-0022's I1 nothing light sees has
  changed since. A chunk without has it from `generated`. The edge clears it at
  E5.
- **A neighbour that is not in the replica passes.** The light was made beside
  that neighbour as generated; if it was changed since, the edge does not know,
  and learns it when the neighbour comes (arrive, step 4).
- **T's one hole in the second draft is closed in section 5**: a job that failed
  left an entry `Carried` and current, which arrive's step 4 then tested without
  looking at the entry's own `as_made`. A failed job now leaves `Fallback`, which
  step 4 never keeps.

| Case | What T says | What the client is shown |
|---|---|---|
| A world as generated by one version | passes everywhere | carried light; no job |
| Light made by other rules than the edge's (`made_by` older or newer) | fails for that chunk | computed by the edge's rules. Its neighbours take it as a witness all the same, through its `beside` |
| **A seam**: a chunk of version 2 beside one of version 1 (ADR-0021, section 6) | fails for both, where both are in the replica | computed from the blocks that are there |
| A chunk S8 made again beside old ones | the same | the same |
| **A chunk stored before `beside` existed** (format 2's first form of light, section 10) | fails for it and for the eight around it, where they are in the replica: its `beside` is 0 and its `generated` is 0, since only changed chunks are stored today | computed. It gets `beside` when it is next saved with light |
| **An imported world**: every chunk's light is `IMPORTED`, `beside` the version the import says its chunks are as generated by | passes where all neighbours are imported and unchanged; fails beside a chunk the import did not have | the official server's own light inside, computed light at the rim of what was imported |
| A chunk with the failure bit (ADR-0021, section 5) | as any other of its version: its light was made from the chunks as they came out | carried |

**The import must give its chunks a version.** If W4's import marks its chunks as
generated by nothing (0), `beside` is 0 and T can pass for no imported chunk:
this record's choice 4 would be void and every join into an imported world 329
jobs. Nothing in the tree or the plan says yet which number the import uses
(`docs/groundwork/terrain-plan.md:465`); the step that builds W4 has to, and this
record asks for one that is not 0 and is not a generator's.

**This departs from ADR-0022's item 4**, which proposed that carried light is
sent only if this edge's own rules made it. Then an imported world is lit anew
chunk by chunk at every join, and the owner's first look at real terrain (W4)
shows Clustine's light and not the official server's. The official server lit
each full chunk beside its neighbours as they are stored, so its light passes
the same test as ours. Both reviews found this right. Choice 4.

**What a raised version costs, and what ends it.** ADR-0021 raises the
generator's version with any change to `clustine-light`
(`docs/adr/0021-generation-is-a-function.md:1042-1051`), and stored chunks stay
as they are (`0021:662-663`).

- **After `clustine_light::VERSION` is raised, every chunk stored before fails T
  on every edge, at every join, for the life of the world**: 329 jobs a join in
  place of none, with no block changed.
- **Along every seam between two generator versions** the chunks on both sides
  are computed for good, also where the newer generator changed no block there.
- **What ends it is not built by this record**: a pass in the store or the
  generation service that makes light again, under the version of the day, for
  stored chunks whose light is still theirs (nothing light sees has changed), and
  stores them, as ADR-0021's S8 makes failed chunks again. It is ADR-0021's kind
  of work and gets a record of its own before the first release of a second
  `VERSION`. **It runs behind**: never between a subscription and its snapshot,
  and never asked for or waited for by an edge; a synced write is 22 ms on the
  nodes this is for. Until it exists a raise of `VERSION` is a known cost that
  the commit raising it names. Choice 14.

The edge's computed light has no version: it lives in one edge's memory and is
written nowhere. Two edges of different builds can show different light for the
same chunk for as long as both run.

### 8. What the client does by itself, and what waits for whom

None of these was observed. **Everything this record and its reviews say of the
game's own code is from memory of the game around 1.20 to 1.21, not from 26.3
and not from anything on this machine.** Each rule that rests on such a
statement is built in its safe form first, and the step that takes the cheaper
form waits for the comparison that settles it.

| # | Guess | Which rule rests on it | Until it is settled the edge | Settled by | The step that waits |
|---|---|---|---|---|---|
| G1 | The client lights a block change by itself when the block update comes | none for rightness; the withholding of section 5 is there in case it does | withholds; a torch is lit by the edge within a job either way | L1's comparison, from the server's side (below); the owner's trial A | none |
| G2 | In a light update a section in neither mask is left as it is | "the differing sections" | **names every listed section in every update** | L1's comparison: if the official server names fewer sections than the chunk lists, the client leaves the rest, or the game would not work | L8 |
| G3 | A section in neither mask of a chunk's packet is, for the sky, read from the one above (`clustine-light/src/lib.rs:37-45`), and dark for block light. Above everything that is "full", which is observed of the official server's packet (`docs/protocol-26.3.md:220-224`) | `Absent` in a chunk's packet | sends `Absent` in neither mask, as the official server does for the flat world. The doubt is under something that floats, and it is the crate's own | **L1's comparison, part (d): the chunk packets a newcomer is sent under a platform in the sky**; then W1 and W4 for terrain | none; if it is wrong, see the outcomes below |
| G4 | The client takes a chunk it has as the chunk's new content, the sections the new packet does not name too | nothing any more: a chunk sent again is unloaded first (section 6) | unloads | not needed | none |
| G5 | The client does not draw a chunk until the neighbours beside it are loaded (`docs/protocol-26.3.md:140`, `400`, from a wiki of 1.20) | nothing for rightness; it hides the rim (section 9) and the hold | does as decided | the owner's trial C | none |
| G6 | The client's own relight at a block change gives what `clustine-light` gives | "the differing sections": a section the edge finds unchanged is not named, and would keep the client's own guess | names every listed section | L1's comparison of **arrays**: the client's engine is the server's (memory), so equal levels from the official server and from `clustine-light` for the same placement is G6 for that case. Then the owner's trial B | L8 |
| G7 | **The official server sends a light update for a chunk only to players for whom the chunk is at the rim of their view; for a chunk inside the view it sends the block update alone** (the second reviewer's memory, and this draft's author's of the same versions) | nothing for rightness. It decides whether L1's comparison sees anything, and it is the question of choice 16: to whom an update goes | sends every update to everyone who has the chunk | L1's comparison, which has an observer of each kind | none |

G2, G4 and G6 are the ones that, wrong, leave light wrong for good; G1, G5 and G7
cost time, bytes or show at the rim. So G4 is closed by a rule and G2 and G6 by
the safe form.

**L1's comparison with the official server** (`official_server`, ignored). The
official server runs the flat world.

- **Who stands where.** Bot A places and breaks. **Observer N** ("near") stands
  across a chunk border from A's placements, with A's chunk well inside its view.
  **Observer R** ("rim") stands so far off that A's chunk is the outermost chunk
  of its view in that direction: with a view distance of 2 that is a few chunks
  away, and the test finds the distance by walking R outwards until the server
  unloads A's chunk and then one chunk back. Each records every `light_update`:
  in which tick against the block update, for which chunks, which masks, and the
  arrays.
- **What A does, in turn**: a torch placed on the ground two blocks from the
  border, and broken; a stone placed at head height under the open sky, and
  broken; a pillar built from the ground to **y = -32, the lowest block of its
  section, with nothing else in the three by three chunks around above the
  ground**, and its top block broken, which leaves the section above without a
  block in its 27; a roof of three by three stones over the torch; and **a
  platform of stone at least three sections above the ground (48 blocks), at
  least five blocks wide and lying across a chunk border, with the pillar it
  was built from taken down again**, so that it floats over sections that
  neither hold a block nor have a neighbour that does. (Whether the bot builds
  it or the test tells the official server's console to is the step's to find
  out.)
- **(a) The test fails, it does not pass, if neither observer was sent a light
  update in the whole run.** L8 cannot rest on a comparison that looked at
  nothing.
- **(b)** It says for each placement **which observer saw what**: N alone, R
  alone, both, neither.
- **(c)** It makes `clustine_light::light` over the blocks the observer holds of
  the nine chunks and compares every array the official server sent with it,
  **level by level, laid over the chunk's light**: an array of zeros is the
  crate's `Dark` and an array of fifteens its `Full` (the official server may
  send either where the crate has the case; memory). It does not compare
  `LightData` whole, as `flat_chunk_matches_the_official_server` does
  (`services/edge/src/encode.rs:354-355`): that is the pattern for a packet with
  one right form, which a light update is not.
- **(d) After the placements a third bot joins** beside A, and the light of the
  chunk packets it is sent, masks and arrays, is compared with
  `clustine_light::light` over the same blocks, for the nine chunks around the
  platform. This is where `Absent` against a section of levels shows; no light
  update can show it, and no chunk of W1's sample has such a place.
- **(e)** It says whether a section that lost its light (the pillar's top) was
  named at all, and how.
- **(f)** It writes what was seen into `docs/protocol-26.3.md`, in place of the
  wiki's notes at lines 220 to 226 and 400.

**What each outcome means.** Each line is what is concluded and what is done; no
outcome is taken as settling what it did not look at.

| Seen | Concluded | What follows |
|---|---|---|
| Neither observer is sent any light update | Nothing. The test fails | L8 does not go ahead. The arrangement is looked at (the view distance, where R stands) before anything is read into it |
| R is sent updates, N none (G7 as remembered) | **G1 holds from the server's side**: the game cannot work otherwise. It is no answer to G6 for chunks inside a view, and it settles G2 only if R's updates name fewer sections than the chunk lists | L5 as built is right and dearer than the game: it sends to all, whole. **Choice 16 is then the owner's**: an update only to a holder who lacks a neighbour of the chunk within the change's reach, or who was sent the chunk with light older than its blocks. Most of section 9's row for a hundred in one place would go |
| N is sent updates too | G7 is wrong for 26.3 | Nothing changes; the server does as this record does |
| An update names fewer sections than its chunk lists | **G2 is settled** without a client | L8 no longer waits for G2 |
| Every update names every listed section | Nothing about G2 | L8 waits for the owner's trial B for G2 as well |
| The arrays equal `clustine-light`'s, level by level | **G6 is settled for those placements** | L8 may go ahead once G2 is settled too; trial B is then a check after the fact (below) |
| An array differs | A fault of `clustine-light`, or of what the bot holds of the blocks | L8 does not go ahead. The difference is found; if the crate is wrong it is mended, `VERSION` and the generator's version are raised (section 7 has the cost), and C4's comparison is run again |
| A section that lost its light is named, as zeros or fifteens | How the official server says what section 6 says by keeping a section listed | Written down; section 6 stands either way |
| It is not named | The official server leaves the client's old levels there | The same |
| (d): the third bot's packets list the sections under the platform as the crate does | **G3 holds for the one case the crate names as unknown** | The sentence in `lib.rs:37-45` is rewritten to say so |
| (d): the official server lists a section the crate has `Absent`, or the other way round, or their levels differ | G3 is wrong there, or the crate's flood is | The crate is mended to give the official server's cases and levels (`VERSION` raised), which is right for the packet too, since the packet is the chunk's light. If only the listing differs, `has_light` (`flood.rs:347-353`) is what changes |

**The owner's trial, with a real client**, written into the roadmap by step L5.
Start with `cargo run --release -p clustine`; the switch is read once at start.
The server serves the flat world, so the trials are built from blocks placed.

- **A (G1).** `CLUSTINE_LIGHT_UPDATES=off`. Build a closed box of stone, five
  blocks each way inside, with yourself in it, and place a torch. *If the room
  lights up, the client lights by itself.* Break the torch: *if it goes dark,
  the same for taking away.*
- **B (G2, G6, and what whole updates cost the client).**
  `CLUSTINE_LIGHT_UPDATES=differing`. Build a tunnel of stone that crosses a
  chunk border (F3+G shows them): place and break a torch one block from the
  border ten times, then a torch on each side; build and take down a pillar of
  forty blocks in the open and watch its shadow. *A band of wrong light along a
  border or a section's edge that stays, and that goes away after leaving and
  coming back, means G2 or G6 is wrong.* Then the same with `whole`: *it must
  not appear.* **And with `whole`, stand beside water that is flowing down a
  slope of placed blocks and watch the frame rate** (F3) against the same place
  with `differing`: *a drop is the client drawing every section of six chunks
  anew four times a second* (section 9).
- **C (G5).** `--view-distance 2`, in a tunnel that runs out of view: *is the
  tunnel's far end, 32 blocks away, drawn, and is it darker within a chunk of
  the rim than after walking one chunk towards it?*

**The switch has three values from L5 on**: `off`, `whole` and `differing`; a
job is told at its dispatch which update to make, so all three cost one packet
a result. `whole` is the default until L8. **L8 does not wait for the owner**
(the work has not waited for a trial since 2026-10-08): it makes `differing`
the default when L1's comparison has settled G2 and G6 as the table says, and
the switch stays until the owner has done trial B. If L1 settles G2 and not G6,
L8 takes the middle form instead: the differing sections **and every section a
block change since the last update sent could have touched** (the section of
the change and the one above and below it for block light; for a change the sky
sees, every section from one above it down), which needs G2 alone, since
whatever the client lit by itself is then sent over. If L1 settles neither, the
default stays `whole` and L8 waits for trial B.

### 9. The cost, and how long light can be stale

A job is taken at **2.2 ms**, which is the main session's figure from step C4
for the sample's chunks in a release build and is not written in the tree; step
L0 measures it again and writes it here. A flying player brings 28 chunks a
second (ADR-0022, section 4). `T` is the number of light threads: two on six or
eight processors, three on twelve. Two threads do about 900 jobs a second.

| Case | Jobs | Thread time |
|---|---|---|
| The flat world; terrain as generated by one version of generator and light rules; an imported world away from its rim | **none** | none |
| A block placed or broken | the chunk and those it reaches: 5.9 at the mean, once | 13 ms |
| A join of 329 **stored** chunks of which none passes T | **329**, and no light update | 0.72 s, 0.36 s on two threads; the nine around the player first |
| A join into terrain **that is being generated**, slower than about twenty chunks a second | **two to four a chunk and one to three updates** (**guess**, the first review's): a chunk is lit against a wall after `LATEST`, sent, and lit again as the chunks of the next column come, which is 17 to 21 chunks later in ascending order | 1.4 to 2.9 s, spread over the time generation takes |
| Flying over terrain in which every three by three holds a change | **about two a chunk** (**guess**): one when it comes at the rim, one when the chunks beyond it come; 56 a second | 123 ms a second a player. **Sixteen such players fill two threads** |
| Standing where water flows in `F` chunks apart from each other | at most 4 a second for each chunk reached: `24 F` a second | `53 F` ms a second. **Nineteen such places fill one thread** |
| A whole view of 329 chunks in flow | 1,316 a second | 2.9 threads: more than a default edge has. The queue grows, chunks someone waits for are served, the rest go stale |
| **A hundred players in one built-up place** | none for a join: jobs are a chunk's, not a player's | no light. **But every update goes to a hundred queues and is compressed on each** (`lib.rs:56-57`): forty chunks changing four times a second at 12 KB an update, whole, are 190 MB a second to compress, more processors than the light. And up to a thousand packets a second for each player towards a queue of 2,048 that drops a player when full (`play.rs:31`), of the order entity moves in a crowd already are |
| **A hundred players entering at once, in one place**, into terrain where no chunk passes T | **329**, the same as for one | 0.72 s; **everyone's chunks are lit 0.36 s after they came**, on two threads. The light is not what costs: each chunk's packet of 32.5 KB is made once and **compressed a hundred times**, 1.07 GB for the join, on the hundred connections' tasks; and a result is offered to up to a hundred waiters, each offer a sort of what that player has pending |
| **A hundred players entering at once, apart**, into such terrain | **32,900** at a view distance of 8 (47,300 at 10, 63,700 at 12) | 72 s (104 s, 140 s). **On two threads the last chunk of the last player is lit 36 s after the join** (52 s, 70 s); on three, 24 s. Nearest first across all players gives **everyone their nine in about a second** (900 jobs, 2.0 s of thread time) once those chunks are in, and everyone's view then fills outwards at the same pace. A chunk's packet is compressed once here |

The last two rows are for terrain that needs jobs. **In the flat world the owner
plays today a hundred players entering cost no job at all**, and after a player
has built, the jobs are those of the chunks around what was built.

**What a whole update costs the client is not in that table, and is a guess.**
From memory, the client marks every section a light update names, and the
sections around it, to be drawn anew. Until L8 an update names every listed
section of its chunk, so a torch is about six chunks times every listed section
on every client that has them, and where water flows it is that four times a
second for each chunk. The table counts the server's bytes and not this. Trial
B asks the owner to watch the frame rate; L8 is what ends it, and section 8
says what L8 can take without the owner.

**After B6 the first line is rare.** 72 chunks in 100 carry a mark or a tick and
most marks are on fluids (ADR-0022, section 7, item 1); once they are honoured,
most chunks of terrain with water or caves come without light, or beside one that
does. Then a join is the third or fourth line and a flight the fifth: for a
hundred players flying apart, 12 processors of light, and a hundred entering
apart wait half a minute for their far chunks on a default edge. This record
bounds the work to one job where the draft of ADR-0022 had nine and puts it off
the fan-out task; it does not make it small. What would: more threads, which is
a setting (`light_threads`); light kept from one job to the next and mended at
a border or a change, which `clustine-light` cannot do; or the pass of section
7 run also after B6's changes have settled. The last two are other records.

**What is measured, where, and what fails:**

| Step | Measured | Fails if |
|---|---|---|
| L0 | A job (`clustine_light::light` over nine chunks of the sample), in a release build and in the dev profile before and after the profile lines of section 10 | no figure fails it; if the release figure is not about 2.2 ms, `REST`, `IN_FLIGHT` and the threads' share are looked at again, and this section's table is made anew |
| L0, L5 | `tools/check.sh`, with the tests' worlds in memory, before L0's profile and after L5 | more than a tenth longer |
| L7 | A join of 329 stored chunks, seven in ten changed | more than 329 jobs; any light update; more than 1.5 times L0's time a job, times 329, of thread time |
| L7 | A join into terrain that is not generated | no figure fails it; jobs and updates a chunk are written down, and if they are above two and one the hold is looked at again |
| L7 | A hundred bots in one place where forty chunks change four times a second | the fan-out's longest turn above 5 ms (**guess**); a bot dropped for a full queue; the processors spent compressing updates are written down, and that figure decides section 11's item 7 |
| L7 | **A hundred bots entering at once in one place**, seven chunks in ten changed | more than 329 jobs; a bot dropped; the fan-out's longest turn above 5 ms (**guess**). Written down: the time until every bot has its nine and its view, and the processors spent compressing |
| L7 | **A hundred bots entering at once, apart**, the same terrain | a bot that has not its nine chunks 3 s after its last of them came (**guess**: a second of hold, a second of jobs, and as much again); a bot dropped. Written down: the time until the last view is whole, on two threads and on a quarter of the processors, and if it is above what a player would sit through, the default of `light_threads` is raised |
| L7 | The fan-out's turns at a join **beside** a hundred players, whose packets are all made anew (section 1) | the longest above 5 ms (**guess**). If making a packet is what fails it, the copying that C5 owes (`encode.rs:66-85`) is done before this record's steps are called done |

All of L7's figures are taken with the worlds in memory, since they judge the
edge; nothing of them is a disk's.

**Memory.** With ADR-0022's means for terrain: a chunk with its light is 32.0 KB
(blocks and biomes 19.8, light 9.3 with `Full` as a case, heights and the rest;
`0022:317-326`) and its packet 32.5 KB (`0022:548`). A view is 329, 473 and 637
chunks at view distances 8, 10 and 12 (`view_area`, `fanout.rs:3145-3161`,
recomputed).

| A hundred players | apart, 8 | apart, 10 | apart, 12 | in one place, 8 to 12 |
|---|---:|---:|---:|---:|
| Entries (100 × the view; one view) | 32,900 | 47,300 | 63,700 | 329 to 637 |
| Chunks and their light (× 32.0 KB) | 1.05 GB | 1.51 GB | 2.04 GB | 10.5 to 20.4 MB |
| Packets, **as the second draft kept them**, until the chunk changes (× 32.5 KB) | 1.07 GB | 1.54 GB | 2.07 GB | 10.7 to 20.7 MB |
| Packets, **kept only while an entry has a waiter** | none once a join is over; during it, those of the batches under way | the same | the same | at most the same 10.7 to 20.7 MB, while the slowest of the hundred still waits |
| This record's fields (× 200 bytes) | 6.6 MB | 9.5 MB | 12.7 MB | under 0.2 MB |

- So the rule of section 1 halves what a hundred players apart cost: 1.05 GB in
  place of 2.12 GB at a view distance of 8, 2.04 in place of 4.11 at 12.
- **Of the 32.0 KB, 9.3 KB is light, and after B6 most of it is light the edge
  made**: most chunks then arrive without (above). "Computed light is the same
  size", as the second draft had it, is true and hid that it is memory the
  chunk did not bring: 0.3 to 0.6 GB of the second row.
- What waits in the connections' queues is the same bytes, shared, and bounded
  as today by 2,048 packets a queue.
- A thread keeps a scratch of 4.2 MB from its first job on. Copies made because
  a job held a chunk at a change: at most nine of 23 KB for each job under way.
- A flat chunk is about 6 KB (`0022:328-329`): a hundred players apart in the
  flat world are 0.2 GB at a view distance of 8.

**How long a client can be shown stale light:**

| After | At most |
|---|---|
| A block change in a chunk that had rested, no job under way | one job, behind the chunks someone waits for |
| A block change that reaches a chunk whose job is under way | that job's result is withheld; the next is dispatched at once: two jobs |
| A block change in a chunk that is changing all the time | `REST` and one job |
| A neighbour that comes while others are awaited | `LATEST` and one job; nobody is sent the chunk in between, and a second player waits that long for a chunk the first is looking at |
| A failed job | until something marks the chunk again. The entry is `Fallback` and counted |
| More marked chunks than the threads work off | not bounded for chunks nobody waits for: the oldest go first after `OVERDUE`, behind everything someone waits for. A line is logged, once a second at most, while the oldest of them is older than `OVERDUE` |

**How long a chunk someone waits for can wait**: `LATEST` and one job in
ordinary play; under lasting overload not bounded for the far chunks of a view
(section 4), and a line is logged, once a second at most, while the entry that
has been in `waiting` longest has been there longer than `OVERDUE`.

**The rim.** A chunk whose neighbour nobody wants is lit, if it is computed, with
a wall there: within 14 blocks of that border, light that would come from beyond
is missing, in caves and under overhangs. The sky's own light is not. The edge
does not ask for a wider ring (ADR-0022, "Ruled out"). At a view distance of 8
that is 128 blocks away and, by G5, not drawn.

### 10. What this needs of other crates

**Of `clustine-world`, which the second draft asked of ADR-0022's step C2.** C2
is built. For each of the five things, as the tree has it:

| # | Asked | In the tree at `96d8ecb` | So |
|---|---|---|---|
| 1 | `Light::beside: u16`, kept through `Chunk::set` with the light, written in format 2 | **Not there.** `Light` is `made_by`, `sky`, `block` (`crates/clustine-world/src/light.rs:95-100`); `Chunk::set_light` builds the light anew from those three (`chunk.rs:462-466`); the file has `made_by` and the two lists (`crates/clustine-format/src/chunk.rs:241-248`, `403-411`); the flat generator sets no such thing (`services/worldgen/src/lib.rs:68-78`) | **Still to be built**, in L0a. A change to ADR-0022's sections 2, 3, 4 and 6 and to ADR-0021: "Changes to earlier records" has the text |
| 2 | "What light sees" is one function, `clustine_light::alike`, with `is_air` in it; `clustine-world` depends on `clustine-light` | **There in another form, and one part missing.** The function is `light_tells_apart` in `clustine-world` (`chunk.rs:148-162`), private, over six fields: emission, dampening, `occludes`, `occludes_by_shape`, the set of occlusion faces, `propagates_skylight_down`. **`is_air` is not among them.** The dependency is the other way round, as ADR-0022's section 6 said: `clustine-light` depends on `clustine-world` (`crates/clustine-light/Cargo.toml`), so the function cannot live in `clustine-light` | The function stays where it is, **becomes public, and gets `is_air`**: L0a. `clustine_light::alike` is gone from this record. A change to ADR-0022's section 1 |
| 3 | `Chunk::take_light(&mut self) -> Option<Light>` | **Not there.** A chunk's light can be read (`chunk.rs:439-441`) and set (`446-467`); only a change that light sees takes it away | **Still to be built**, in L0a. A change to ADR-0022's section 2 |
| 4 | `sky_light` stays in `clustine-world` | **There as asked**: `light.rs:108-145`, exported (`crates/clustine-world/src/lib.rs:15`), and what `encode.rs:96` reckons with | Nothing |
| 5 | A way to give a chunk light after its blocks are set (`Chunk::with_light`) | **There in another form**: `Chunk::set_light` (`chunk.rs:446-467`) is public and does it; the format's reader and the flat generator use it (`clustine-format/src/chunk.rs:407-411`; `worldgen/src/lib.rs:75`) | Nothing. The tests call `set_light` after their last `set` |

**Why `is_air` belongs among what light sees.** The flood asks a section whether
it holds a block that is not air (`flood.rs:133`), and a light section is listed
only if it or one of the 26 around it does (`flood.rs:347-353`). A state that
differs from air in nothing else `light_tells_apart` compares, set as the first
block of a section, makes sections of nine chunks stop being `Absent`, and the
chunk keeps light that says otherwise. Everything else the flood reads of a
state is covered: dampening and emission (`flood.rs:58-72`), and the faces
through `occludes`, `occludes_by_shape` and the face set, which two states share
exactly when all six faces are equal
(`crates/clustine-data/src/block_states.rs:319-323`; `faces.rs:18-31`). Which
states differ from air in `is_air` alone was not looked up; test 1 finds them.

**Of `clustine-light`** (additions; `VERSION` is not raised, since no chunk's
light changes):

```rust
/// Which of the eight neighbours (north-west first, row by row, without the
/// centre: the order of `Around::neighbours`) a change in column `x`, `z` of a
/// chunk can alter the light of, by distance alone.
pub fn reach(x: usize, z: usize) -> [bool; 8];

/// What a thread keeps between calls, so that a call allocates nothing large.
/// A call assumes nothing of what an earlier call left in it.
pub struct Scratch { /* the grid and the levels */ }

/// As `light`, and for each section named in `also` that comes out `Absent`, the
/// case its levels have (never `Absent`).
pub fn light_with(
    around: &Around<'_>,
    also: Unlisted,
    scratch: &mut Scratch,
) -> (Light, UnlistedLevels);
```

**Of the workspace's `Cargo.toml`**: `[profile.dev.package.clustine-light]`,
`[profile.dev.package.clustine-world]` and `[profile.dev.package.clustine-data]`,
each `opt-level = 3`, which the test profile inherits.

- The second draft had the first of the three, and the second review's finding 6
  showed that it would not have optimised the part of a job that reads the
  blocks, which was generic over the reader and so compiled with the edge. **The
  trait is gone, and that reason with it**: `Grid::read` takes the nine chunks
  (`flood.rs:100`) and is compiled with `clustine-light`.
- **The problem remains in another form.** What a job reads block by block is
  `clustine-world`'s: `Section::states` (`section.rs:352-354`) calls the
  section's own `get` for each of 4,096 blocks, up to 9 × 24 sections a job, and
  `Section::uniform_state` and `may_hold` (`flood.rs:134-143`); and what it asks
  of a state is `clustine-data`'s (`Cell::of`, `flood.rs:58-72`; `faces.rs`).
  Neither is generic or marked to be inlined, so by the Cargo book's rule
  ("Overrides and generics") each is compiled with its own package's settings:
  without optimisation, whatever `clustine-light` is given. This is read from
  the rule, not measured; the 15 ms against 2.2 are the only measurement, and
  they are of all three packages unoptimised.
- So the override names all three. `clustine-world` and `clustine-data` are also
  what the simulation's tests spend their time in; optimisation changes no
  result of theirs. What it does to the time of a build is part of what L0
  times.
- **What is measured where**: L0 takes the time of a job in a release build and
  in the dev profile, before the lines and with one, two and three of them, on
  the sample world through `crates/clustine-light/tests/official.rs:419-421`,
  and writes the figures here. The lines that buy nothing are left out. The
  end-to-end tests run real edges, and after L5 every block a bot places is six
  jobs, on machines where the clusters of processes already share the
  processors: at 15 ms that is 90 ms of thread time a block.

**Of `services/edge/Cargo.toml`**: `clustine-light` among the dependencies. That
changes `Cargo.lock`, which is an input of `tools/datagen/generated.sums`
(`tools/datagen/src/sums.rs:21`), so the sums are made anew in the same commit,
as `080f82c` did for the last such change. It is in L0a, the main session's,
because `cargo datagen` needs the jar and a subagent downloads nothing.

**Of `clustine-protocol`**: `LightUpdate { chunk_x, chunk_z, light: LightData }`
(`docs/protocol-26.3.md:226`; id 49, `packet_ids.rs:198`). The bots take it and
keep, for each chunk they have, the light their packets add up to.

**In `services/edge/src/light.rs`** (new):

```rust
pub(crate) struct LightJob {
    pub basis: u64,
    pub position: ChunkPos,
    /// North-west first, row by row; the centre, the fifth, is always there.
    pub nine: [Option<Arc<Chunk>>; 9],
    pub sky: bool,
    pub old: Option<Arc<Light>>,
    /// Whether the update is to name every listed section (`owed`, or the switch
    /// of section 8), or those that differ from `old`.
    pub whole: bool,
}

pub(crate) enum LightOutcome {
    Lit {
        light: Arc<Light>,
        /// The update for those who have the chunk; none if it would name nothing.
        update: Option<Bytes>,
    },
    Failed { why: String },
}

pub(crate) enum FromThread {
    Started { basis: u64, thread: usize },
    Done { basis: u64, position: ChunkPos, outcome: LightOutcome },
}

/// Pure: the same job gives the same result. Panics are the caller's to catch.
pub(crate) fn relight(job: &LightJob, scratch: &mut Scratch) -> LightOutcome;

/// Where jobs go: threads, the dispatching turn, or a test.
pub(crate) struct Lighting { /* a sender of jobs */ }
```

`encode.rs`: `light_data` (`encode.rs:91-135`) becomes a function of the two
lists of light sections and of which sections to name, which `chunk_packet`
calls with all of them and `relight` with those of its update. In L3
`chunk_packet` (`encode.rs:25`) takes the light beside the chunk, and its two
callers in `fanout.rs` (`152`, `5915`) change with it. (The second draft added a
second function beside `chunk_packet` in L2, which nothing would have called
until L3.)

### 11. ADR-0022's items, one by one

| Item | Answer |
|---|---|
| 1 (a) keep computed light with the replica's chunk | Yes: section 1 |
| 1 (b) take a result that is older by block changes | Taken for those who wait, withheld from those who have the chunk; the packet is made from the blocks as they are, not from the job's snapshot: section 5 |
| 1 (c) drop carried light only at a change light sees | Done in ADR-0022 and built (`chunk.rs:287-289`); this record adds `is_air` to what light sees and `beside` so that the kept light can be used: section 10 |
| 1 (d) nearest a waiting player first | Section 4: before everything else, by a rank that follows the views |
| 1 (e) a figure that fails W3 | Section 9; step L7 |
| 1, weighed: the region makes B6's changes before the chunk is handed out | The first step is in the first snapshot anyway, which is the chunk after its tick (ADR-0012, rule 11). The flow goes on for seconds and across chunks, so nothing an edge is handed is settled, and a region cannot hold a chunk back until it is. Not done |
| 2 a player who has the neighbour and not the changed chunk | A change marks every neighbour it reaches that has a chunk, whoever views it; a chunk that comes changed marks its neighbours (arrive, step 4). L is the standing rule |
| 3 nine computations and eight updates | Held: section 4. Test 5 counts; section 9 says where it does not hold |
| 4 versions | T, section 7; imported light is kept, against the proposal; the cost of a raise is named |
| 5 revisions; a failed job; empty batches; results for chunks that left; the copies | One counter for the edge (section 1); section 5, with a kind of its own for what a failed job leaves; section 6; section 5; one copy of the light and a packet only while someone waits (section 1) |
| 6 a light update cannot say `Absent` | A section once listed stays listed, as its levels: section 6 |
| 7 the packet compressed once | **Not done here, and measured with the cases that matter.** For a chunk at a join of one player a connection compresses once either way. For a light update that a hundred players are sent, and for a chunk that a hundred who enter together are sent, it is a hundred compressions of the same bytes (section 9). L7 measures both; if it is processors, compressing once on the light threads is the next record, and with it 1 (b) as proposed. Choice 7 |
| 8 not checked by anyone | Section 8 says which step waits for which answer; the time of a job is measured in L0 |

### 12. The steps

ADR-0022's C1 to C5 are on `main`. **Every commit leaves the four commands of
`CLAUDE.md` green**, `cargo clippy --workspace --all-targets --locked -- -D
warnings` among them, and the comparisons with the official server equal. Three
rules make that hold where the second draft's steps did not (the second
review's finding 7):

- **A step adds only what it reads.** A field, a set or a variant comes with the
  step whose code reads it. What a step writes and only later steps act on, it
  reads in the check at the end of a turn (`fanout.rs:512-517`), which is
  compiled into every build and run in every build with debug assertions; that
  check grows with each step.
- **`light.rs` is compiled for tests alone until L4**: `#[cfg(test)] mod light;`
  in `lib.rs`. Everything in it is used by a test of the file, or it is not in
  L2. L4 takes the attribute away.
- **`script.rs` grows with what exists**: its regions and its client in L2t, its
  end of `Lighting` in L4. Each part comes with tests of its own that use all of
  it.

**From L2t to L8, the 201 tests of `fanout.rs` and the random runs against the
model in `scenarios` stay green at every commit.** Where a step changes what one
of them asserts (an unload before a chunk sent again, in L3u), the test changes
in that commit and the commit's message names it.

**Six steps touch `fanout.rs`: L2t, L3u, L3, L4, L5 and L8. They are the main
session's and are not delegated.** Nothing that is delegated touches that file.

| # | Step | What holds afterwards, and what the step must do for that | Touches `fanout.rs`? | Delegated? |
|---|---|---|---|---|
| L0a | **The shared types, first**: `Light::beside` (set by the flat generator, kept by `set_light`, written as the light byte's case 2, `docs/world-format.md:106`); `Chunk::take_light`; `light_tells_apart` public and with `is_air`; the notes in ADR-0022 and ADR-0021 and the changed numbers in the plans; `services/edge` depends on `clustine-light`, with the sums made anew; **the time of a job measured** in a release build and in the dev profile, and the profile lines that the figures bear out. `tools/check.sh` timed before | Everything. A world of format 2 made before is read as it was. Nothing uses the additions yet; they are public items of library crates | No | **No**: shared types, the lock file, the jar. Pushed before anything is delegated |
| L0b | `clustine-light`: `reach`, `Scratch`, `light_with` | Everything; nothing uses them yet | No | **Yes**: one crate. Test 1 by someone else |
| L1 | `LightUpdate` in the codec; the bots add it up; **the comparison of section 8 with the official server, run, and what it saw written into `docs/protocol-26.3.md` and into section 8's table of outcomes** | Everything; the edge sends none yet. The comparison fails if no observer saw an update | No | The codec and the bots: yes. Running the comparison: no |
| L2 | After L0b is on `main`. `light.rs`, compiled for tests alone: `relight`, the `Around` over nine `Arc`s, the difference and the update, the threads with `Started` and the scratch made anew after a panic, the end of `Lighting` that runs where it is called. `encode.rs`: `light_data` as a function of the lists and of which sections to name | Everything; the server's build does not contain it yet. `chunk_packet` gives the bytes it gave | No | **Yes**: one new file and one function changed. Test 2 by someone else |
| L2t | **The tests are made ready before any rule changes**, in both modules and the model. (a) The tests' client decodes a chunk's packet and reads the block that tells its state, in place of comparing bytes (`fanout.rs:5911-5917`), and passes over a light update. (b) The harnesses' chunks (`fanout.rs:4133-4145`, `6098-6135`) carry light with one version and one `beside`, given by `set_light` after their last `set`, so that T will pass for them. (c) `services/edge/src/script.rs`, `#[cfg(test)]`: the scripted regions and a client that keeps blocks and light and can be given a queue of any size (as `join_with_room` gives one today, `fanout.rs:15093-15097`), with a page of documentation and tests of its own | The 201 tests pass unchanged in what they assert | **Yes** (its tests) | **No** |
| L3u | A chunk sent again is unloaded first; `replace`; block updates to `sent` and `replace`; a view that moves away unloads what is in `replace` | The tests whose client is sent a changed chunk at a resume see an unload before it, and the model expects one; they are changed in this commit and named in its message | **Yes** | **No** |
| L3 | The replica as far as it is read without jobs: the chunk in an `Arc` with its light taken out and kept beside it, `kind` with the two cases `Carried` and `Fallback`, `as_made`, `shaped` and `lit`, T at arrive, `waiters` and `ready` with rule R1 of section 4, the packet kept only while there is a waiter, E2 keeping the entry's chunk, `chunk_packet` taking the light. **No jobs**: an entry that fails T keeps the light it came with or gets `sky_light`, as `Fallback`, and at E5 gets `sky_light` over its heights as they are now, as C5 has it | As C5 for every client. The check at the end of a turn holds the moments, `waiters` and `ready` to sections 1 and 4, in tests every turn | **Yes** | **No** |
| L4 | Jobs: `changed`, `job`, `unsendable_since`, `marked_since`, `retouched`, `dispatched`, `queued` and the three sets; the kind `Computed`; dispatch, held, rank kept up in `shift_view`; results for those who wait; the send rule with R2 and R3, no empty batch; failed jobs, threads written off, no thread left; **E5's marks**, so that a chunk in the replica is not left with the light of its arrival. **`FanoutConfig` gets `light`**: the server's values in `lib.rs`, **`latest` and `rest` of nothing in both harnesses of `fanout.rs`**, which also get the end of `Lighting` that runs in the dispatching turn. `mod light` loses its `cfg(test)`; `script.rs` gets the end that holds jobs, with a test that holds and answers one. Those who have a chunk are still sent nothing about light | A join into changed terrain is lit right, one job a chunk; a later arrival is sent a changed chunk with right light. **In the old tests a job is met at a block change and at every arrival beside a changed chunk**, and each is done within its turn: such a chunk is sent at the end of the turn it came in, after that snapshot's entities and not before them. Trial A can be done | **Yes** | **No** |
| L5 | Light updates to those who have the chunk, **every listed section named**; withheld results, `owed`, the exemption from the rest; the switch of section 8 with its three values. The roadmap gets the owner's trials A to C | L holds. ADR-0022's finding 2 is closed. The old tests' client passes over the updates | **Yes** | **No** |
| L6 | Test 4, written from this record and `script.rs`'s page by someone who has seen neither `fanout.rs` nor L3 to L5, in `services/edge/src/light_orders.rs`. **Its author starts once L4 is on `main`**, when the interface for holding and answering a job has been compiled against; it is run against L5 | What it finds is mended before L7 | No | The test: **yes** |
| L7 | Section 9's measurements. The plan's three checks for W3 with bots | W3 is done | No | No |
| L8 | The default of the switch becomes `differing`, or the middle form, as L1's comparison allows (section 8); the switch goes when the owner has done trial B | Smaller updates, and less for the client to draw anew | **Yes** | **No** |

## Changes to earlier records

Each gets a note under its own "Changes" heading that points here, with this
text. The notes and the edits are in this record's commit and in step L0a, so
that what is built from ADR-0022 from now on is built with them.

**ADR-0022, section 1**, the sentence of "The light is valid" that lists what
light sees ("… the light they give, the light they take, whether and with which
faces they stop light, whether the sky passes down through them:
`light_emission`, `light_dampening`, `occludes`, `occludes_by_shape`, the set of
occlusion faces, `propagates_skylight_down` …") becomes:

> … the light they give, the light they take, whether and with which faces they
> stop light, whether the sky passes down through them, **and whether they are
> air**: `light_emission`, `light_dampening`, `occludes`, `occludes_by_shape`,
> the set of occlusion faces, `propagates_skylight_down`, `is_air`. Whether a
> state is air is among them because which light sections a chunk and its
> neighbours list turns on which sections hold a block that is not air
> (ADR-0023, section 10). The test is one public function,
> `clustine_world::light_tells_apart`, and an edge asks it too.

**ADR-0022, section 2**, "**Light**: `Light { made_by: u16, sky:
Vec<SectionLight>, block: Vec<SectionLight> }`" becomes `Light { made_by: u16,
beside: u16, sky: Vec<SectionLight>, block: Vec<SectionLight> }`, and the list
under it gains:

> - `beside` is the version of the generator that made the nine chunks the light
>   was made from, or 0 where whoever made the light did not say.
>   `clustine_light::light` gives 0; whoever calls it for a chunk it has
>   generated sets `beside` before `Chunk::set_light`, which keeps it. It stays
>   with the light through every change that keeps the light, also after
>   `generated` has become 0. An edge reads it (ADR-0023, section 7).

and the section gains, after `Chunk::set`:

> **`Chunk::take_light(&mut self) -> Option<Light>`** takes the light out of a
> chunk and leaves the chunk without. An edge keeps the light beside the chunk
> (ADR-0023, section 1). Nothing else calls it.

**ADR-0022, section 3**, the row "Light" of the body ("u8: 0 for none, 1 for
present. If present: `made_by` u16; then for sky and then for block, per light
section bottom to top: u8 0 absent, 1 dark, 2 full, or 3 followed by 2,048
bytes") becomes:

> u8: 0 for none, 1 for present without `beside`, 2 for present with it. If
> present: `made_by` u16; for 2, `beside` u16; then for sky and then for block,
> per light section bottom to top: u8 0 absent, 1 dark, 2 full, or 3 followed by
> 2,048 bytes.

and the list under the table gains:

> - **The light's `beside` is a case added within format 2** (ADR-0023), as
>   ADR-0020's kinds were added within format 1. The writer writes 0 or 2. The
>   reader reads 1 as light whose `beside` is 0, so every world of format 2 is
>   read as it was, and refuses any value above 2. (Until then the reader took
>   every value but 1 for "none", `crates/clustine-format/src/chunk.rs:403`.) A
>   build from before this reads a chunk written with 2 as one without light and
>   then fails on what follows; worlds are not carried back.

`docs/world-format.md:106` and the head of `crates/clustine-format/src/chunk.rs`
(line 30) get the same row.

**ADR-0022, section 4**, gains after its first paragraph:

> The light travels with its `beside` (ADR-0023). That changes the shape of the
> three messages by one field; the wire number is raised once more for it, if it
> is there by then.

**ADR-0022, section 6**, the table "Who calls it", gains a sentence under it:

> **Who says `beside`** (ADR-0023): the generation service, its generator's
> version; the flat generator, `FLAT_VERSION`; the import of an official world,
> the version it says its chunks are as generated by, which must not be 0. An
> edge's own light is never put into a chunk and says nothing.

and its first line, "Pure, depending on `clustine-world` and `clustine-data` and
nothing else", stands: ADR-0023's second draft asked for the dependency the
other way round, and its third does not.

**ADR-0022, section 2**, "(the plan's B6, under ADR-0023)" (line 295) becomes
"(the plan's B6, under ADR-0024)".

**ADR-0021, section 5**, "`Lit(p)` is the chunk a request is answered with. It
carries "as generated by version *n*" and, where it applies, "with a failure"
(below)." gains:

> Its light says `beside` = *n* (ADR-0022, section 2, as changed by ADR-0023):
> the version of the generator that made **this** chunk, also where one of the
> nine it was lit from is a stored chunk of an older version (section 6). An
> edge that has both then finds the neighbour to be of another version and
> computes the light of both (ADR-0023, section 7); were `beside` the
> neighbour's version, the edge would send light made beside blocks that are no
> longer what the neighbour's version says.

(Whether ADR-0021 lights a new chunk from a stored older neighbour or from that
neighbour made again in memory was not traced; the sentence is right either
way.)

**The numbers.** This record is ADR-0023. Every mention of a later record's
number that has to change, found by searching the repository for "ADR-0023" to
"ADR-0029":

| Where | Says | Becomes | The record meant |
|---|---|---|---|
| `docs/adr/0022-what-a-chunk-carries.md:295` | ADR-0023 | ADR-0024 | neighbour updates and scheduled ticks |
| `docs/groundwork/terrain-plan.md:526` | ADR-0023 | ADR-0024 | the same (step B1) |
| `docs/groundwork/replicas-plan.md:635`, `638`, `647` | ADR-0024 | ADR-0025 | greetings, the term and the elector |
| `docs/groundwork/replicas-plan.md:881` | ADR-0024, and ADR-0026 | ADR-0025, and ADR-0027 | the same, and the store's standby |
| `docs/groundwork/replicas-plan.md:456`, `639`, `715` | ADR-0025 | ADR-0026 | balance by load (R4) |
| `docs/groundwork/replicas-plan.md:495`, `640`, `726`, `786` | ADR-0026 | ADR-0027 | the store's standby (R5) |

Nothing else in the repository names a record above 0022: not the roadmap, not
`docs/architecture.md`, not the code. **One more record is waiting for a
number**: the draft for fewer syncs in turn
(`docs/groundwork/few-syncs-in-turn-draft.md:3`, "to be numbered"). Whichever of
it and the records above is committed next takes the next free number, and the
table is then one further on.

## Tests

1. **`clustine-light` and `clustine-world`**, by someone else. For random worlds
   and a random block: replacing it by a state that `light_tells_apart` does not
   tell from it leaves the light of all nine chunks equal, **cases and levels**;
   replacing it by any state leaves equal the light of every neighbour `reach`
   does not name, unless a section went from air to holding a block or back;
   `light_tells_apart` is true for stone and air, air and a torch, water and
   air, glass and stone, a slab's two halves, **and for air and every state that
   is not air**, of which the test lists those that differ from air in nothing
   else; false for stone and an ore, grass and dirt, air and cave air.
   `light_with` gives `light`'s result with a fresh scratch, with one used for
   another world before **and with one a call left half way**, and, for a
   section that is `Absent`, the levels a slow sweep gives. `beside` and
   `take_light`: light keeps its `beside` through `set_light`, through a change
   light does not see, through the file and through a message; a file of format
   2 with the light byte 1 is read with `beside` 0, and one with 3 is refused.
2. **`relight` and the threads**, by someone else. Its light equals
   `clustine_light::light` over the same nine chunks, but for sections the old
   light listed, which stay listed with the sweep's levels; the update names
   exactly the sections that differ, for each pair of cases, or every listed
   section if the job says so; decoding it over the old light gives the new; no
   update when nothing differs. A job whose neighbour is of another height,
   which makes the flood panic (`flood.rs:121-124`), gives `Failed` from the
   threads, the thread goes on, **and the next job on that thread gives the
   right light**. A thread says `Started` before each job. A job that never
   returns (the thread's loop is handed the function it runs, and the test
   hands it one) is given up `JOB_PATIENCE` after its `Started`, its thread and
   no other is replaced, **and a job that waited behind it in the queue is not
   given up**; with no thread left a job fails without being queued. The
   threads end when the sender is dropped; the nine chunks are bit for bit as
   before.
3. **The edge, by its author**: each row of the tables of sections 3, 5 and 6;
   that the sets and counts are what section 4 says after every turn.
4. **The edge, from this record, by someone else**, with `script.rs` on a paused
   clock and with `latest`, `rest` and `job_patience` set by the test. **Its
   brief says**: a job the test holds lets the paused clock run to
   `JOB_PATIENCE` at the first wait on a client's queue, so the script answers
   before it waits or sets the patience longer than anything it waits for. A
   model beside it keeps the replica as the test fed it and, for each client,
   the light its packets add up to. It drives, each as a test of its own and
   then in random orders from a seed:
   - a chunk first, then its eight neighbours, in every position of the changed
     one among them; the neighbours first; one neighbour never; one after
     `LATEST`;
   - E5 before the dispatch, between dispatch and result, after the result; two
     in one `TickDelta`; one in a neighbour within reach and one out of it; one
     that fills an empty section; a torch between dispatch and result with a
     client that has the chunk: **that client is sent no light until the next
     job's, and then every section**;
   - E4 at each of those points, **and the same change said a second time by
     another region: it raises nothing**;
   - E1 at a neighbour, E3 at the chunk, E6, and E6 then E1, each between
     dispatch and result;
   - **E2 between dispatch and result, from the region that served the chunk
     and from another one** (after a merge, and after `NotMine`): the job's
     result is taken as if nothing had come;
   - **E3 twice for one chunk before its light is there**: one unload, one
     chunk, the second snapshot's blocks; **E3 and then the view moves away
     before the light is there**: the client is sent the unload it is owed, and
     nothing of the chunk after it;
   - a result that fails, with and without `owed`, **for an entry that was
     `Carried` and for one that had no light**: those who wait are sent the
     chunk, those who have it nothing, the entry is `Fallback` and counted, no
     job is made for it until it is marked, a neighbour that then comes gives it
     a job, and the result of that job names every section; a job that is never
     answered; **a job with no thread left**; results in another order than the
     jobs;
   - **a waiter whose queue is full when the result of a chunk is fanned out,
     with a second waiter behind them: the second is sent the chunk; and the
     same for one who has the chunk, at a light update, with a second behind;
     and the same where the player who is removed was the last viewer of the
     entry itself, of an entry in `waiting`, and of an entry whose job is under
     way**;
   - **two results taken in turns that follow each other, for chunks beside
     each other, where the first's fan-out removes a waiter of the second**;
   - **forty chunks that change at every tick and a player who joins beside
     them, with one slot: every chunk the player waits for is dispatched before
     any of the forty** (the first review's finding 1);
   - **with one slot and more waited-for chunks than it serves, a player who
     flies towards lower coordinates and one who flies towards higher: for each,
     among the chunks they wait for, one nearer to them as they now stand is
     dispatched before one farther, and of two equally near the older** (the
     second review's finding 3);
   - a second player who comes to have the neighbour of a changed chunk and
     then the chunk (ADR-0022's finding 2), and the other way round; a player
     whose view leaves a chunk and returns while its job is under way;
   - a resume that brings the same chunks (E2) and one that brings a changed
     one: an unload before the chunk, block updates in between;
   - a section that is listed, becomes `Absent`, and whose levels then change
     (a roof widened far above): **it is named again**;
   - a batch confirmed while everything pending waits for light.

   **What it can see.** From outside the task a test sees packets and jobs, not
   turns. So it checks **on the stream of packets**, as each comes: L3 (no chunk
   from an entry that the model says was not sendable), that no batch was empty,
   that no light update went to a client without the chunk, that an unload came
   before every chunk sent again, and that every job's nine chunks were the
   model's replica at its dispatch. It checks **where the script has made the
   edge quiet** (every job it holds answered, then `settle` and `sync` as the
   old harness has them, and no job since): L1 for every client and chunk; and
   L2: light of kind `Carried` where T passes on the model, otherwise the light
   of the last job taken for the chunk, which was dispatched after the chunk's
   last mark, otherwise what a failed job leaves. And L4 in its own words: with
   every job answered at once and no view moving, a pending chunk is sent by the
   time the clock has passed `LATEST`, after no more jobs than there are chunks
   someone waits for and events in its three by three.
5. **Counts**: for a view of 25 chunks that come in ascending, descending and
   random order, all changed: 25 jobs and no light update; with one neighbour
   after `LATEST`: its neighbours' jobs once more and one update each at most.
6. **End to end, with bots** (the plan's W3): no job is made in an untouched
   world (the edge's count of jobs, published as the count of players is,
   `lib.rs:232-233`); a block placed by one bot gives a later arrival the right
   light, and gives a bot that has the neighbouring chunk a light update; no job
   runs on the fan-out task (a debug assertion in `relight` on the thread's
   name, off for the end that runs in the turn); **and the edge's count of
   failed jobs is 0 at the end of every such test**, so that a run in which a
   job was given up after five real seconds on a slow machine says so.
7. **With the official server** (`official_server`, ignored): section 8's
   comparison in L1; after L5, the same placements against Clustine give light
   updates whose arrays, laid over the chunk's light, equal the official
   server's, and a third bot's chunk packets the same light.
8. **Measured** (L0, L7): section 9's figures.

## What a player notices

- **Light under trees, in caves and under what players build is right**, also
  after blocks change, and also across a chunk border for a player who did not
  have the changed chunk.
- **A torch lights its room at once**, by the client if G1 holds and by the edge
  within a job if not, and is not put out again by older light.
- **While a chunk waits for its light the client has nothing of it**: a chunk is
  never sent first with its blocks and then with its light, and never with
  light that is known to be missing. Where the chunk will be, the client shows
  what it shows for any chunk that has not come: nothing, the sky or the fog
  behind it (**guess**). The one exception is a chunk the client already has
  and that is sent again after a link was lost: it stays on the screen as it
  was until the new one is ready.
- **In ordinary play that wait is not seen.** In the flat world, and in terrain
  as it was generated, every chunk comes with its light and is sent at once, as
  today. Where blocks were changed, a chunk at the rim of the view waits for
  the chunks beside it that the same step brought, and for one job: some
  milliseconds, 128 blocks away at the default view distance. **What is sent
  first, and when it is corrected**: a chunk with carried light is sent at once;
  if a chunk then comes beside it that was changed, the first is corrected by
  one light update, within a second and a job, and the correction reaches at
  most 14 blocks in from that border. At a block change the block update goes
  first and at once; the light follows within a job, or, where blocks keep
  changing, within a quarter of a second and a job.
- **At a join into terrain where water has flowed**, chunks come nearest first
  and each a few milliseconds later than its blocks arrived. At the frontier of
  generation a chunk can wait up to a second for a neighbour, and its light at
  the border can change once or twice as the chunks beyond it come.
- **When a crowd enters at once into such terrain**: a hundred in one place
  notice nothing of light, since a chunk is lit once for all of them, a third of
  a second after it came. A hundred **apart** each see the chunks around them
  within about a second and then their view fill outwards over half a minute on
  an edge with two light threads, where without light to make it would fill as
  fast as the regions answer. It does not happen in the flat world. The lever
  is `light_threads`, and L7 measures it with bots before anyone sees it.
- **A player can be shown in a chunk the client does not have yet** for the time
  it waits: entities go by what is wanted (`fanout.rs:2975-2979`), as they do
  today while a batch is outstanding.
- **A second player who arrives where a first one stands** can wait up to a
  second for a chunk the first is looking at, if a chunk has just come beside it.
- **Where much water flows at once**, light behind the flow lags by a quarter of
  a second, and by more when the threads are behind; chunks nobody has yet are
  not kept waiting by it. Until L8 every update names every section, and the
  frame rate beside the flow may show it (**guess**; trial B).
- **Where far more is asked than the threads do** (many players flying over
  changed terrain), a player sees the chunks nearest them come first wherever
  they fly, and the far ones late.
- **After a link was lost and blocks changed meanwhile**, the changed chunks are
  taken away and put back in one step (a flicker is a **guess**).
- **If the light rules fail for a chunk**, it is shown lit as chunks are lit
  today, by columns, or with the light it had before the change: dark where it
  should not be, never missing. The server's log and its counts say so.
- **Nothing of this waits for the disk.** On the owner's nodes a synced write is
  22 ms; no chunk's light is read from or written to a file by an edge, so what
  a player sees of light is as fast on network storage as in memory.
- **What the owner is to try**: section 8's A to C; and with two clients: a torch
  in a tunnel at a chunk border, seen from both sides; the same placed while the
  second client is far away and then walks there; taking down the top of a tower
  built into empty sky; a platform built high in the sky and a look at the
  ground under it; a roof of glass replaced by stone; water let down a slope; a
  join beside all of it.

## Ruled out

| What | Why not |
|---|---|
| Light computed when a chunk is about to be sent (the draft of ADR-0022) | ADR-0022's finding 1: after B6 that is every chunk at every sending |
| A revision counter in each entry | An entry is removed and made anew and its counter comes again (`fanout.rs:2337-2358`) |
| Jobs that read the replica, or a lock around it | The replica is one task's; a lock would make every turn wait for a flood |
| Copying nine chunks into each job | 200 KB a job, 67 MB at a join, on the fan-out task; nine pointers do it |
| tokio's blocking pool | Its number of threads is not the edge's to set, and a test's paused clock runs ahead of it |
| The overdue before the waited-for (the first draft) | The first review's finding 1: a join that is never served |
| Dispatch and offers at the end of every turn (the first draft) | The first review's finding 4: thousands of turns a second, each over the marked and the players |
| A stale result sent to those who have the chunk (the first draft) | The first review's finding 3: it puts out light the client has made |
| Leaving out of a stale result the sections a change could have touched (the first review's other way) | A list of sections a job for every chunk in reach; withholding is one bit |
| A stale result thrown away | Where water flows a player would wait for a chunk for as long |
| The packet from the job's snapshot, then the block updates since | Section 5: the same on the client's screen, a list a chunk and a second order of packets more |
| Holding a chunk until its light is current | It never is where water flows |
| Holding a chunk with carried light for its neighbours | Every chunk of every join would wait |
| `region_patience` for a neighbour that does not come | 20 s of eight missing chunks around one the store cannot read |
| A relight when a neighbour leaves | Section 3 |
| **The hold and the rest as constants (the second draft)** | The second review's finding 1: the 201 tests and the random runs meet them at their first block change |
| **Rank taken once and ties by position (the second draft)** | The second review's finding 3: holes near a player flying west or north while their far rim is lit, and low coordinates served first |
| **An age after which a waited-for chunk goes first whatever its distance** | It bounds the wait of a far chunk at the cost of holes near a flying player, which is what a player sees; choice 12 |
| **A failed job that leaves the entry's kind as it was (the second draft)** | The second review's finding 5: wrong light called current and carried, and kept through the next neighbour |
| **The entry's light sent whole to those who have the chunk after a failed job (the second draft)** | It is light from before the change, sent over the client's own |
| **Every job given up after `JOB_PATIENCE` when no thread is left (the second draft)** | Seven minutes for a join; it fails at dispatch |
| **An index of who has a chunk** | A hundred look-ups a result do it (section 4); the entry would carry a second set to keep right at every removal |
| **The packet kept until the chunk changes (today, and the second draft)** | A gigabyte for a hundred players apart, never read again (section 9) |
| **Format 3 for `beside`** | It would refuse every world made since C3 for two bytes that a new case of one byte carries; choice 5 |
| **`clustine-world` depending on `clustine-light` (the second draft)** | The tree has it the other way round, as ADR-0022 decided, and the function is where `Chunk::set` is |
| **`#[expect(dead_code)]` on what a later step will use** | It is unfulfilled in the test build, where the code is used, and `clippy --all-targets` builds both |
| Remembering for each client which sections it was sent as levels | Section 6: a section that stays listed does the same with no state a player |
| Every section listed in a packet that replaces a chunk | 17 arrays of 15s a chunk, 34 KB; an unload is five bytes |
| Jobs on threads for the old harnesses' chunks | Their chunks carry light that passes T, and what their block changes mark is lit within the turn |
| Reading the client's jar | The comparison settles G2, G6 and G7 for a server's part and G3 for the case in doubt, the unload closes G4, and the owner's trial is asked for anyway |
| Light mended from a change (an engine with queues, as SteelMC's) | 9,268 lines there (`docs/groundwork/terrain-generation-check.md:206`) for what a flood does; it is what cuts section 9's cost, and a record of its own if L7 says it is needed |
| Light made anew by a worker or the store **in the way of a load** | ADR-0022, "Ruled out"; and a synced write is 22 ms. Behind, as section 7's pass, it is another record |
| Computed light stored by the edge | An edge has no world to write to, and nothing of light may wait for a disk |
| The edge asking for a wider ring | ADR-0022; section 9, "The rim" |
| Making and compressing the packet once, in this record | Section 11, item 7 |

## Risks

- **The cost after B6** is most chunks lit by the edge: section 9. Every figure
  is 2.2 ms times a count; the 2.2 ms are one run's on one machine, and how many
  marks change a block nobody knows (ADR-0022, "Not checked").
- **A hundred players entering apart wait half a minute for their far chunks**
  on two threads, in terrain that needs jobs. It is measured in L7 before it is
  seen, and the default of `light_threads` is what gives.
- **Under lasting overload the far chunks of a view have no bound**: nearest
  first is all that is promised (section 4).
- **A raised light version relights the stored world at every join**, and a seam
  stays computed, until the pass of section 7 exists.
- **A join into terrain that is being generated** does not get one job a chunk.
- **A hundred in one place**: the updates and the compressing, not the jobs, and
  the queue of 2,048. While updates are whole (until L8) they are two to five
  times the bytes, and the client draws every section named (**guess**).
- **A player who joins beside others has every packet made anew** on the fan-out
  task, since packets are no longer kept; and making one still builds a palette
  for every section (`encode.rs:66-85`). L7 has a figure that fails it.
- **L1's comparison may show that the official server sends light updates to
  far fewer players than this record does** (G7). Then L5 is right and dear, and
  choice 16 is open.
- **Two relights a chunk in flight** is a guess; a view is not a square and its
  rim moves in steps.
- **Those nobody waits for wait behind the waited-for without a bound.**
- **`reach` and `light_tells_apart` are arguments.** Both reviews found them
  sound against the code; test 1 sweeps random worlds. If light depends on
  something they leave out, a chunk keeps wrong light and nothing marks it.
  `is_air` was such a thing, and was found by reading.
- **Removal from inside a send** is the kind of order this file loses. R1 to R3
  are rules for the author; test 4 has the orders, written by someone else.
- **In the old tests a chunk beside a changed one now follows its snapshot's
  entities** where it went before them. No test was found to assert that order;
  none was looked for line by line. L4's commit names any that does.
- **L3 to L5 change `take_snapshot`, `change_block`, `send_chunks`, `shift_view`
  and `remove_player`**, beside the hand-over and the resume. They add no
  message to a region and change no subscription.
- **An unload before a chunk sent again** changes what the client is sent at a
  resume, and may show.
- **A section that stays listed** is light the official server would not send;
  a comparison of packets after such a change differs there, by design.
- **A worker and an edge of different builds** can disagree on
  `light_tells_apart`.
- **`is_air` among what light sees** makes chunks drop their light at changes
  that kept it until now, in the simulation and the store too. That is right,
  and it is more chunks without light.
- **Chunks stored before `beside`** cost nine jobs each at every join until they
  are saved again with light.
- **The import must name a version**, or imported light is never used.
- **Generation must be a function for T to be sound** (ADR-0021, section 10).
- **A thread that hangs is left behind**, four at most, and after `4 + T` the
  edge lights nothing.
- **Three packages are optimised in test builds**, if L0's figures bear all
  three out: a slower build of two crates everything depends on.
- **The switch of section 8** is code that exists for a trial, until the owner
  has done it.

## Not checked

- Anything a client does: section 8. Everything said of the game's own code is
  memory of other versions.
- What the official server sends at a block change, and to whom: L1.
- The time of a job, by this draft's author: the 2.2 ms and 15 ms are the main
  session's. What each of the three profile lines buys: L0.
- Which crate's settings a function is compiled with, beyond the Cargo book's
  rule; no build was made for this draft.
- The time `Arc::make_mut` and making a packet of 32 KB take on the fan-out task.
- Whether a flight makes two jobs a chunk; the pace at which a region answers
  from the generator.
- How many sections a relight changes at a border.
- What the client shows between an unload and the chunk that follows it, and
  where a chunk in the middle of its view has not come.
- Whether any of the 201 tests asserts a chunk before the entities of its own
  snapshot for a chunk beside a changed one (section 12, L4).
- Which states differ from air in `is_air` alone.
- Whether ADR-0021 lights a new chunk beside a stored neighbour of an older
  version as stored, or made again.
- Which version W4's import gives its chunks.
- How the platform of L1's comparison is built on the official server.
- The Nether and the End: no sky, other heights. `sky` is in the job for them.
- `bin/clustine/tests` and `tools/botswarm` beyond the lines cited; ADR-0013 and
  ADR-0015 beyond what `fanout.rs` does with `served_by`.

## Choices the owner has not settled

Each is decided above so that the record can be built from; each can be turned.

1. **Light is beside the chunk at the edge, one copy**, and the chunk in the
   replica carries none.
2. **A chunk with current carried light is sent at once**; only chunks that need
   a job wait for their neighbours.
3. **A neighbour that leaves marks nothing.**
4. **Imported light is used under the same test as ours**, against ADR-0022's
   reviewer.
5. **`Light::beside`**, an addition to ADR-0022's chunk, **written as a new case
   of one byte within format 2**, so that no world made since C3 is refused;
   and **`is_air` among what light sees**. The other way is format 3 and the
   refusal ADR-0022 has for format 1.
6. **A result under which a block changed is withheld from those who have the
   chunk**, also when the change was in a neighbour within reach, which is one
   case more than the session named.
7. **The packet is made from the blocks as they are, and compressed once a
   connection**; L7 measures the hundred, in one place and entering.
8. **The settings**: `REST` 250 ms, `LATEST` 1 s, `OVERDUE` 5 s, `JOB_PATIENCE`
   5 s from the moment a thread takes the job, a quarter of the processors and
   two at least, twice that many jobs under way, four threads written off at
   most.
9. **A failed job leaves light of a kind of its own, sends the chunk to those
   who wait and nothing to those who have it, and is not tried again** until the
   chunk is marked; **an edge whose threads are all gone goes on** with today's
   light, at once for every chunk, and does not stop.
10. **A section once listed stays listed for the entry's life**, in place of a
    memory for each client.
11. **A chunk sent again is unloaded first.**
12. **Chunks someone waits for before everything, nearest first by a rank that
    follows the views, and no bound** on a far chunk under lasting overload nor
    on stale light behind the waited-for. An age that puts a long-waiting chunk
    first is the other way.
13. **Updates name every section until L8**, and L8 goes by L1's comparison
    without waiting for the owner's trial B, which then checks it.
14. **A raised light version costs a relight of the stored world at every join**
    until a pass that is another record.
15. **This record is ADR-0023**, the one for neighbour updates ADR-0024, and the
    replicas plan's three are ADR-0025 to ADR-0027.
16. **To whom a light update goes.** This record sends it to everyone who has
    the chunk. If L1's comparison shows the official server sending it only to
    players for whom the chunk is at the rim of their view (G7), the owner may
    want the same: an update only to a holder who lacks a neighbour of the chunk
    within the change's reach, or who was sent the chunk with light older than
    its blocks. That rests on G1 and G6 for everyone else, where today's rule
    rests on neither. It need not be decided before L5.
17. **The packet is kept only while someone waits for the chunk**: half the
    memory for players apart, and every packet made again for a player who joins
    beside others.
18. **Three packages optimised in test builds**, if L0's figures bear them out.

## Review

Two independent reviewers went over this record, one after the other, each
against the tree of the day and each without building or running anything. Each
finding was checked against what it cites before it was taken.

### The first review

Of the first draft, against the tree at `12e445f`. Thirteen findings. It found
sound, and they stand: a neighbour leaving marks nothing; moments from one
counter and the rule for taking a result; `reach` and what light sees; held and
rest; T with `beside` and imported light under it; no empty batch; the bounded
queues; the packet from the blocks as they are.

1. **The order of dispatch could keep a waiting player's chunks from ever being
   lit, and L4 was not true.** Accepted. Chunks someone waits for go first
   always; `OVERDUE` orders the others alone; the hold has a time of its own
   that is never set anew; test 4 has the join beside forty changing chunks.
2. **The edge's tests would not keep their order.** Accepted. The harnesses'
   chunks carry light that passes T, the old harnesses' lighting runs in the
   dispatching turn, and the client decodes. All of it before any rule (L2t). A
   sendable chunk is offered inside `take_snapshot`, before the entities, as
   today.
3. **A stale result sent to those who have the chunk puts out the client's own
   light.** Accepted. Withheld, `owed`, the next job exempt from the rest and
   sent whole. **Wider than proposed in one thing**: a change in a neighbour
   within reach withholds too (choice 6).
4. **The end of every turn was not bounded.** Accepted. Three sets, `waiters`,
   `ready`, the events on which dispatch and offers run: section 4.
5. **L1 failed for a section that is `Absent` after being sent as levels; G4.**
   Accepted. The section stays listed. **Answered otherwise than the session's
   words** in that the memory is the entry's and not each client's (choice 10).
   A chunk sent again is unloaded first.
6. **The guesses were settled after the steps that rest on them.** Accepted.
   Section 8 says for each guess what waits. **Not followed**: reading the
   client's jar, which the session replaced by the comparison and the trial;
   and listing every section in every chunk's packet as G3's safe form, which
   is 34 KB a chunk against a doubt that is the crate's own.
7. **A raised light version turns the stored world into computed light.**
   Accepted. Named with its cost and what ends it, which is another record and
   runs behind every load.
8. **The steps.** Accepted, all five parts.
9. **The flood without optimisation.** Accepted: a profile, timed before and
   after.
10. **A job that never answers, with one thread.** Accepted: two threads at
    least, a thread written off is replaced, the threads end with the fan-out
    task.
11. **Cost the table left out.** Accepted, with one precision: a call
    **allocates** 4.2 MB and holds 4.2 MB at its peak.
12. **One job a chunk holds only while chunks come faster than `LATEST`.**
    Accepted; a row of section 9 and a measurement of its own.
13. **Smaller orders.** Accepted: block updates go on to a client whose chunk
    waits to be replaced; entities against their chunk and the second player's
    wait are under "What a player notices"; test 4's brief has the paused clock.

Nothing was rejected outright. Three things were answered otherwise than
proposed, each named above.

### The second review

Of the second draft, against the tree at `f9801d6`, before ADR-0022's C2 to C5
were built. Eight defects, eight unclear points, seven nits; it went through
the first review's thirteen and found nine answered, three answered in part and
one named as asked. Its verdict: **accept after the listed changes; no third
review needed.** It found sound, and they stand: that no chunk is sent before
its light is right, twice or never, in every order it tried but the two its
findings name; results for chunks that changed or left; no light update before
its chunk; T, but for one hole; `reach`; the rim; the packets against the
protocol notes.

This draft was then written against the tree at `96d8ecb`, in which C2 to C5
and ADR-0020 are built. Every citation was read again there.

1. **The edge's existing tests meet the hold and the rest.** Accepted, as
   proposed: the two are settings, nothing in both old harnesses, set by L4; the
   sentence about what the old tests meet is corrected (sections 4 and 12).
2. **`waiters` and `ready` were not kept where a player is removed, and removal
   is entered from every send.** Accepted: rules R1 to R3 of section 4, the
   order of a result's fan-out in section 5, the check's clause, and the orders
   in test 4.
3. **Rank was frozen and ties went by position.** Accepted, the first of the
   review's two ways: (rank, the moment put in), rank kept up in `shift_view`,
   with its cost. **And** the two sentences the review said to strike under its
   other way are struck all the same: the waited-for go nearest first, and a far
   chunk has no bound under lasting overload (sections 2, 4, 9; choice 12).
4. **L1's comparison could come back empty, and G3 was put to no comparison.**
   Accepted, all four parts: an observer at the rim, a test that fails on
   silence, a third bot whose chunk packets are compared under a platform in
   the sky, and what each outcome means. The reviewer's memory of whom the
   official server sends an update to is G7, a guess like the others, and its
   question for the owner is choice 16.
5. **A failed job left wrong light called current.** Accepted, as proposed: the
   kind `Fallback` always, never kept by arrive's step 4, nothing sent to those
   who have the chunk and `owed` set, a job that fails at dispatch when no
   thread is left; and the bound for a join follows (section 5).
6. **The profile did not optimise what reads the blocks.** The finding's reason
   is gone with the trait `Blocks`; **the problem is there in another form** and
   the fix is another one: three packages named, measured in the dev profile
   (section 10). The review's `uniform_section` for the adapter is what
   `flood.rs:134-146` now does for the chunks themselves.
7. **Three steps did not pass `clippy`.** Accepted, the first of the review's
   two ways: a step adds only what it reads; `light.rs` is compiled for tests
   until L4; `script.rs` grows in two steps; L6's author starts after L4. **Not
   followed**: `#[expect(dead_code)]`, the review's second way, which the test
   build would not fulfil. The second function beside `chunk_packet` and the
   fourth end of `Lighting` are gone, since nothing would have called them.
8. **What is asked of ADR-0022 and ADR-0021 was not a change to them.**
   Accepted: section 10 says for each of the five what the built tree has, and
   "Changes to earlier records" has the text. **Otherwise than the review
   supposed** in two things the code decides: `beside` goes into format 2 as a
   case of the light's byte, since format 2 has been written since; and the
   function for what light sees stays in `clustine-world`, since the dependency
   is the other way round.

The unclear points: **U1** the packet is kept only while an entry has a waiter,
and section 9 has the totals; **U2** two rows and two measurements for a
hundred entering at once; **U3** the second set has its right name and its key,
`marked_since`; **U4** what a whole update costs the client is in section 9 as
a guess, trial B asks for the frame rate, and L8 has the middle form to take
without the owner; **U5** the two walks over the players are said; **U6** a
thread says which job it has, the scratch is made anew after a panic, test 6
asserts no failed job; **U7** what test 4 can see is said, and the three orders
and the change said twice are in it; **U8** `as_made` from `beside` whoever
made the light, "it can be darker", and `enter_by_arrival`'s order.

The nits: **N1, N2, N3, N7** every line and count is of `96d8ecb`, which is
past the review's own table; **N4** a closed box of stone; **N5** arrays laid
over the chunk's light; **N6** a result carries one update, chosen at dispatch.

Nothing was rejected. Three things were taken otherwise than proposed, each
named above: the profile in finding 6, `expect` in finding 7, the format and
the dependency in finding 8.

**Found in the tree while this draft was written, by neither review:**

- A snapshot that shows the same replaces the replica's chunk and keeps the
  packet made from the earlier one's light (`fanout.rs:2413-2414`). E2 now keeps
  the entry's chunk and light.
- `light_tells_apart` has no `is_air` in the code as built
  (`chunk.rs:151-162`): the second review had it of ADR-0022's text.
- The format's reader takes any light byte but 1 for "none"
  (`clustine-format/src/chunk.rs:403`).
- Making the edge depend on `clustine-light` stales the generated data's sums
  (`tools/datagen/src/sums.rs:21`), so that step cannot be delegated.
- The packet's blocks are still built through a palette made anew
  (`encode.rs:66-85`), which the rule for kept packets makes matter.
- The second draft's "a diagonal neighbour for 91 columns of 256" is each
  diagonal's count; some diagonal is reached from 244. The mean of 5.9 stands.
- A whole light update is about 12 KB, not 10: in a packet a full section is
  an array (`encode.rs:150-153`).
- The import has no version to put into `beside` yet.
- The draft for fewer syncs waits for a number too.

## Found while building

**L0a and L0b (2026-10-13).** The time of a job on the sample world, the median of 36
chunks:

| Build | A job |
|---|---|
| Release | 2.15 ms; 2.10 ms in a scratch that is used again |
| Dev profile, no line | 15.6 ms |
| Dev profile, `clustine-light` optimised | 5.3 ms |
| … and `clustine-world` | 3.3 ms |
| … and `clustine-data` | 2.5 ms |

Each of the three profile lines buys a clear step, so all three are in the
workspace's `Cargo.toml`; the light crate's own tests take 1 s where they took 16.
A scratch is 4.2 MB for 24 sections, since the levels of the sky and of blocks are
never alive together. `reach` is by 14 steps sideways, and the flood's own table of
distances is what it reads. 7,266 of the 35,723 states differ from air in nothing
light sees but in being a block, glass among them: `is_air` among what light sees is
not a corner case. Two helpers the record did not name: `Unlisted::listed_in(&Light)`
and `UnlistedLevels::put_into(&mut Light)`. `Unlisted` is two words of 64 bits, sky
and block apart.

**L1's comparison with the official server (2026-10-13)**, run alone on a quiet
machine; what it saw is in `docs/protocol-26.3.md` under "Update Light". By section
8's table of outcomes:

| Guess | Outcome |
|---|---|
| G7: an update goes only to a player for whom the chunk is at the rim of the view | **Holds.** 217 updates to the client at the rim, none to the client beside the change, none to the builder |
| G1: the client lights a block change by itself | **Holds from the server's side**: the game sends a client that has the chunks around a change nothing but the block update |
| G2: a section in neither mask of an update is left as it is | **Settled**: every one of the 217 updates named fewer sections than its chunk lists |
| G6: the client's own relight gives what `clustine-light` gives | **Holds for these placements** as far as a server can show it: the levels sent were the crate's in 77 of 77 sections (one section listed by one side alone), and a latecomer's chunk packets had the crate's light in all twelve chunks |
| G3, under something that floats | **Holds**: the latecomer's packets list the sections under the platform as the crate does |

**What follows, decided here; the owner may turn it** (choice 16 of "Choices the owner
has not settled"):

- **L5 sends a light update only to a holder for whom the chunk is at the rim of the
  view**: one who lacks a chunk beside it that the change reaches. Everyone else has
  the block update and the chunks around it, and lights the change as the official
  client always has. This is what the official server does, it is far less to send
  (the hundred in one place of section 9 are sent nothing where every one of them
  was to be sent every result), and it rests on G1 and G6, which the comparison
  bears out.
- **Updates name the differing sections from L5 on.** L8's switch and its wait for a
  trial are not needed for G2; the switch stays as section 8 has it until the owner
  has tried trial B, with `differing` as its default.
- A chunk that is **sent** (to a waiter, or again) carries the edge's light as
  before: that is the latecomer's case, and there the official server's light was
  the crate's.
- What is not shown by a server and still wants a real client: that nothing is seen
  to be wrong at the rim of a view while a chunk beside it is missing (trial A).

**L3 to L5 as built (2026-10-14).** The steps were cut smaller than the table of
section 12 has them, each green:

- *L3*, in two parts: the chunk behind a shared pointer with its light beside it,
  and E2 keeping the entry's chunk; then `waiters`, with the packet kept only while
  someone waits and the check at the end of a turn holding both.
- *L4, the part that makes light right*: the moments, `kind`, `as_made`, the test T
  at arrive, change with `reach` and with the first or last block of a section, the
  entries that are not current in one set (`marked`), dispatch at the end of a turn
  with the chunks someone waits for first, a result taken only if its job is still
  the entry's, a failed job leaving `Fallback`, chunks sent only from sendable
  entries and no empty batch. Its second part, the settings of section 4 and
  what they order, came after the tests of L6 and is described below.
- *L5*, by the comparison's outcome above, with one rule more than it names: **a
  chunk that came or was replaced beside one a player has sends that player the
  light of the chunk they have, wherever in their view it is** (`reshaped`). No
  client knows of such a neighbour by itself: a lamp in a chunk that comes later
  lights the chunk beside it. After a block change the update goes only to a holder
  who lacks a chunk beside the one it is for. A result withheld sets `owed` if
  anyone was to be sent it, a job that failed sets it always, and the next job's
  update then names every section.
- `script.rs` came after L5 and not in L2t: the scripted regions, a client that keeps
  blocks and the light its packets add up to, a model of the replica, and an end of
  `Lighting` that holds jobs for the test to answer.

**One thing the rule for rims shows in the tests**: a block that is the first in its
section reaches all eight chunks around it (which light sections are listed turns on
it), and a player who has the changed chunk and all eight is at the rim for each of
those eight, so they are sent eight small updates for them and none for the chunk
itself.


**Found by the tests written from this record (L6, 2026-10-14).** Forty-four tests
of the orders of events (`services/edge/src/light_orders.rs`), by someone who had
this record and the scripted regions and clients and had not read the fan-out. Three
of them failed for what the code did, and all three are mended:

- *A player was left with a block unlit.* A waiter may be sent a chunk whose light is
  older than its blocks (section 6: light of the right shape is sendable). Their
  client was never told of the block that changed, so it has not lit it by itself,
  and the rule for rims then kept the job's result from them unless they lacked a
  chunk beside it. A torch placed a moment before they came stayed dark for them
  until something else changed there. **A chunk sent while it is not current now
  sets `reshaped`**, so that the result that is on its way goes to everyone who has
  the chunk. This is the one a player would have seen.
- *A packet outlived the light it was made of.* When a chunk came beside an entry
  whose light then no longer held, the entry was marked and its packet kept; the
  check at the end of a turn has "a packet only if sendable". The packet is dropped
  there.
- *After a job failed while nobody had the chunk, the next update named only the
  sections that differ from the light of the heights*, which whoever was sent the
  chunk in between has, but which was never the light the difference is to be taken
  from. `owed` is set after every failed job.

**What the record left unclear to its reader**, each settled here as the code has it:

- Section 6 and the tests of section "Tests" still say an update goes to every
  holder; the comparison above changed that. Where the two differ, "Found while
  building" holds.
- *"Lacks a chunk beside it" is judged when the result is taken*, not when the block
  changed: a neighbour that came in between makes the holder one who has all eight,
  and its coming has set `reshaped` in any case.
- *`owed` is of the entry, not of a player.* Someone sent the chunk whole after it
  was set is sent every section once more with the next result. That costs bytes
  and nothing else.
- *`beside` of a changed chunk* is written only by a job's result. Arrive does not
  touch the versions of an entry whose light does not hold.
- The scripted model's light is of the chunks the replica has; a neighbour that
  left is not in it, as it is not in a job.

**L4's second part as built (2026-10-14).** `LightSettings { latest, rest, overdue,
job_patience }` in `FanoutConfig`: 1 s, 250 ms, 5 s and 5 s in the server; nothing
held, nothing resting and a patience without end in both old harnesses and in a
script, unless a test says otherwise. The three sets are `LightQueues`, with
`unawaited` twice, by rank and by age; an entry says which it is in (`queued`), and
one function, `requeue`, puts an entry where it belongs now and is called wherever
what that turns on changed. The check at the end of a turn holds every entry to its
set, and the rank of a waited-for entry to the nearest of its waiters as the views
stand. Where it differs from sections 4 and 5:

- **No tick of 50 ms.** The task sleeps until the first hold or rest ends or the
  first job is to be given up, and never wakes for light sooner than 50 ms after it
  last did. While nothing is held, rests or is with a thread there is no timer.
- **Held is judged when an entry is put into a set**, and again when a chunk comes
  beside it or its time ends. A wanted chunk beside it that stops being wanted
  (its last viewer walked off) does not end the hold; the time does, at most
  `LATEST` later. A chunk that begins to be wanted beside an entry already in
  `waiting` does not hold it.
- **`view.ready` is not built.** An offer is made to the waiters of an entry that
  has just become sendable, so there is something to send them, and the batch is
  made of the sendable among the pending before anything is sorted. The counter
  would save the walk over the pending of a player who has a batch outstanding or
  nothing sendable, which the first two lines of the offer already leave out.
- **A thread written off ends by itself if it ever comes back**: it is told so, and
  takes no further job. Threads are numbered without reuse, so what a thread said
  late is never taken for the word of the one started in its place.
- **With no thread left one job may be out**, and fails where it is dispatched. The
  line for it is the line of every failed job: an error a second at most, with how
  many failed since the last, how many in all, and how many entries have fallback
  light now. **Nothing is published beside the count of players**: the edge has
  nowhere to publish numbers yet, and the log is where an operator reads this until
  it has.
- **Exempt from the rest, once in a rest**, is counted from when the exemption was
  last granted.

Its tests are mine (`services/edge/src/light_policy.rs`: the hold and its end by a
chunk coming and by time, the rest, the exemption, the overdue, the patience and
a late answer; `light.rs`: a thread written off, replaced, and none left), beside
the one of L6 that waited for it (nearest first as the view stands, then oldest).

**Found by the tests of the times written from this record** (`light_times.rs`,
fifty tests by someone who had not read the code): a chunk replaced while it was held
began its hold again, so one replaced more often than once a `LATEST` while a
neighbour stayed away was never lit. A chunk that takes the place of one that could
not be sent either now keeps the start of its hold, as arrive's step 4 says. What the
record left unclear to them: that the age of a chunk in `waiting` is when it was put
in, which for a held chunk is when its hold ended; and the order among chunks of
equal rank and age entered in one turn, which is by position.

**L7 as measured (2026-10-14).** `bin/clustine/tests/lights.rs`, written from
section 9 by someone who had not built the edge, runs the six rows on a fresh
`clustine --seed 12345` each, its world in memory, in an optimised build on a
quiet machine of twelve processors (three light threads). The edge counts what
it does (`services/edge/src/figures.rs`). Two settings exist for this and
nothing else, both read from the environment: `CLUSTINE_LIGHT_AS_CHANGED`
takes chunks in so many tenths of the world, by a fixed hash of their place, as
changed chunks come from the store, without light and not as generated; and
`CLUSTINE_LIGHT_THREADS`. Seeded terrain is made by the preview generator on the
store's one thread, again whenever a chunk is loaded: 17 ms a chunk, 5.6 s for
a view of 329, and **every time below includes it**.

| Row | Measured | By the limits of section 9 |
|---|---|---|
| One join, seven in ten changed | 330 jobs for 329 chunks, 819 ms of thread time (2.5 ms a job); one light update; the nine in 3.3 s, the view in 5.2 s | over: one job and one update. A chunk was made more than the second of the hold after its neighbours had been sent, was lit again with them, and its neighbour's light changed. With stored chunks, which come together, it would not be |
| One join where nothing was made, all changed | 1.00 jobs and no update a chunk; the chunks came at 66 a second | holds; the case of chunks that come slower than 20 a second was not reached |
| A hundred in one place, forty chunks changing four times a second | within everyone's view: 15,982 jobs, 39 s of thread time in 61 s, no update, longest turn 4.0 ms. At the rim of their view: 36,280 jobs, 87 s of thread time, 986,506 updates (3.4 GB before compression), compressing them 64 % of one processor, longest turn 2.8 ms | holds |
| A hundred entering at once in one place, seven in ten changed | 330 jobs; the nine in 3.4 s, the view in 5.4 s; compressing 32,900 chunk packets 11.5 s, 210 % of one processor; 329 packets made in 85 ms | holds at 4.2 ms in one run; over in six others, at 7 to 14 ms (below) |
| A hundred entering at once, apart, seven in ten changed | the last view whole after 623 s on two threads and 631 s on three; the longest any chunk waited at the edge 1.06 s | holds (3 s). The time is the generator's: 32,900 chunks at 18.5 ms on its one thread is 610 s |
| A join beside a hundred | 329 packets made in 75 ms, the view in 0.28 s, longest turn 5.0 ms | holds, barely |

- **The turn of 7 to 14 ms when a hundred enter together is not light's.**
  Timed part by part, it is the snapshot of the chunk they enter in: its chunk
  took 95 µs and showing the hundred players to each other 12.2 ms, about 9,900
  packets in one turn. The 5 ms of section 9 was a guess for light's work, which
  stays under it. Entities are ADR-0012's, and that turn is once a crowd.
- **Making packets is not what costs** (row 6): 0.23 ms a chunk packet. The
  copying C5 owes (`encode.rs:66-85`) is not needed for this record's steps.
- **Section 11, item 7: compressing once is not needed yet.** At its worst, forty
  chunks at the rim of a hundred players changing four times a second, compressing
  updates takes two thirds of a processor, and within everyone's view nothing,
  since nobody who has a chunk's neighbours is sent its light. Most of the bytes
  are whole updates (every section named until L8); L8 makes them smaller.
- **What a player notices is the generator**, not light: 3.3 s to the nine
  chunks at a first join and 5.2 s to the view, and ten minutes for a hundred
  entering apart, where light at the edge waited at most a second for any chunk.
  More light threads change nothing there. The generation service of ADR-0021
  (W5) is what shortens it; until then the preview makes chunks on one thread.
