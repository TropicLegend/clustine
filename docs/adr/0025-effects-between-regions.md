# ADR-0025: Effects between regions

- Status: Proposed on 2026-10-16, from the plan of phase E
  ([e-plan.md](../groundwork/e-plan.md)), after two independent reviews of that plan
  whose findings are worked in (its sections 8 and 9) and the owner's answers to its
  questions (its section 6). Three reviews of this record found it not ready; their
  findings are worked in (see "Review"), with lockstep, which the owner asked for
  afterwards (section 8). A fourth, focused review accepted it after the changes it
  names, which are made. **Accepted** on 2026-10-16; not built. Where the plan and this record differ, this record holds.
- Date: 2026-10-16

## Context

Since phase B ([ADR-0024](0024-block-behaviour-in-a-region.md)) blocks behave: updates
run on, ticks are scheduled, fluids flow, sand falls, leaves decay, things grow. A
chunk ticks only where it and its eight neighbours are loaded in its region, and an
update whose target lies outside the region's loaded land is kept as a seam entry
until one region holds both sides (that record, section 5). At a pinned border
(`--pin`) that is for good: water stops a chunk short of the line on both sides, a
torch whose support is broken across it stays, nothing along it takes random ticks.
After a hand-over between regions that follow players the same holds for the half
minute and more until the emptied region is absorbed.

Nothing passes between regions today but a player's actions, through the player's
edge ([ADR-0012](0012-the-tick-on-chunks.md), sections 2.3 and 2.4). Each region has
its own tick clock; synchronised ticks were rejected
([ADR-0003](0003-partitioning.md)). A region reads a block it has not loaded as
air. The world store orders what matters across regions: the table of chunks, the
records of merges and splits, the log every region commits to
([ADR-0011](0011-the-world-store-and-regions.md)).

The owner asked on 2026-10-15 for updates and ticks to be carried between regions
after phase B, so that a border is seamless, and answered the plan's questions on
2026-10-16: everything of phase B crosses, at every border; through copies of the
neighbours' border chunks and the store; an effect into land nobody uses is dropped,
as the game drops an update into a chunk that is not loaded; and crossings were to be
asynchronous, with lockstep between busy neighbours left for later. Asked then "is
there a way to not lose ticks?", the owner chose lockstep while a border ticks, so
that a scheduled tick crossing a line runs on the tick it would in one region.


## Decision

### 1. What crosses, and how late

Every update, scheduled tick, spread of a fluid and set of a block by a rule whose
target lies in another region's land is carried to that chunk's holder and done there
by the holder's own rules. Each region stays deterministic: a tick is a function of
the region and its inputs, and what comes from across is an input, ordered.

How late:

- **While the two regions' border ticks they are coupled** (section 8): a scheduled
  tick that crosses runs on the tick it would run on in one region, so water, lava and
  sand cross a line at their own pace; an update (`neighborChanged`, `updateShape`),
  which the game runs within the same tick, runs exactly one tick late.
- **Otherwise**, and when a coupling has been given up because a partner stopped
  (section 8), an effect crosses one to a few ticks late, its delay counted from when
  it is taken.

What still differs from one region, and is accepted (the owner, question 1): two
chains that meet on the line may interleave otherwise; a chain that zigzags across the
line (a row of fences along it, leaves decaying across it) takes a tick for each
crossing, where the game takes one tick for all; ticks due on the same tick on both
sides of a coupled line order otherwise than by when each was scheduled (section 8);
and a rail placed at the line just as
its neighbour across changes may stay curved towards a rail that does not answer
(section 4).

An effect into a chunk **nobody uses** (section 5) is dropped, as the game drops an
update into a chunk that is not loaded (the owner, question 5). A region's own update
into land it holds or has asked for is not an effect: it stays a seam entry, as in
ADR-0024 (section 6).

### 2. The stream and sequences

The store numbers every durable group of commits with a **sequence**, increasing for
the life of the world (a group's position in the log, kept increasing across segments
and restarts). The records of merges and splits and the groups that write effects
forward take sequences as groups of commits do. The commit thread sends each region **one stream**, in sequence order:
for each durable group, the effects filed for chunks the region uses, the copy updates
of its copies, its coupled partners' progress (section 8), and then a **mark**, "through
sequence S". A region with no copies, no coupling and no effects gets no marks.

An effect is numbered by its group's sequence and its index in the group, unique and
increasing. A copy update carries its group's sequence and the sequence of that chunk's
previous change, so that a gap is visible.

### 3. Border copies

A region keeps a **border copy** of every chunk that lies in the 3x3 of a chunk it
holds with a **viewer's or a guest's ticket**, and that is `Foreign`, or `Unknown` and
not wanted. Ring chunks do not tick or get post-processed (ADR-0024, section 2), and
copies beside them are not taken. A chunk the region has asked for or loads is not
copied; a copy is dropped when its chunk is loaded.

- A copy is read where a loaded chunk is read. It is never written, ticked,
  post-processed, saved, settled or shown to an edge from this region, and holds no
  ticket.
- **Its base** is the chunk's committed form as of a sequence: the stored chunk with
  its holder's commits since replayed into it, as a restore's chunks are, the stored
  chunk alone where nobody holds it (land returned), or the generator's chunk where none
  was ever made, with the load's rule for chunks made again over (`remakes`), so that
  a copy is what its holder loads. The commit thread lists the log entries and registers
  the copy at the end of a group, whose sequence the base carries; the thread for
  chunks makes it. A
  stored chunk that cannot be read is answered `Unreadable`, and the copy counts as
  absent.
- **Copy updates** are the block changes of each later commit in the chunk. The worker
  keeps updates that arrive before their base and applies those above the base's
  sequence once it has it. A based copy is **current through** the last mark of the
  stream. A gap (an update whose previous change the copy has not had) asks for the base
  again; so does the region's own restart, and a restart of the store, which ends
  every runner (a lost store handle ends it); nothing else does.
- Copies survive the region's beginning anew after a merge or a split, as warm chunks
  do; the absorber keeps its own and asks for those along its new borders, the
  absorbed region's among them; a split's part asks for those along its line. Copies go
  when no viewer's or guest's chunk's 3x3 reaches them.

With copies a chunk along a border ticks (its 3x3 is loaded or copied) and is
post-processed by its holder, and an update beside the line has its six neighbours to
read.

### 4. Effects out

The sim turns a write or schedule into a chunk that is `Foreign`, or `Unknown` and not
wanted, into an **effect** addressed to that chunk: in `set_block_updated`,
`schedule_tick`, the updates' `run_one` and `shape_update` (where a seam entry is made
today), and the fluids' spread. Every read of a block goes through one path that knows
loaded chunks and copies; these read or write chunks directly today and each either
takes copies or names why not: the placement view `Loaded` (`region.rs`), the falling
block's `fall` and `schedule_tick`'s write (`region/ticks.rs`), `keep_seam` and
`loaded_around` (`region/updates.rs`), `update_ticking`, the random ticks (which only
pick chunks that tick, so never a copy), and saving and settling chunks (which skip
copies).

The effect carries the **call** where the target's side decides:

| In one region | The effect | The holder |
|---|---|---|
| `neighborChanged`, `updateShape` | target, direction, kind, source block, flags, depth left | runs the update by its rules |
| `scheduleTick` | position, block or fluid, delay, priority, the tick it was made in (the maker's number) | schedules it, due as the store converted it (section 8) |
| `FlowingFluid.spreadTo` | target, direction, the source's block and fluid state | runs the target side whole: `canSpreadTo` (`canBeReplacedWith`, `canPassThroughWall`, `canHoldFluid`) and `getNewLiquid`, reading the carried source where it reads the source's position, and its own land and copies elsewhere (a position neither loaded nor copied reads as air, as at the edge of loaded land today); then `beforeDestroyingBlock`, the set and its updates. It needs no copy of the source's chunk |
| a rail turned towards a placed rail, a chorus flower or vine grown into a block, fire spreading, a block removed by a rule | target, expected block, state | sets it only if the target is still the expected block, then runs that change's updates |

The placing rail decides its own shape on its copy, as the game decides on what it
sees; nothing in the game reshapes a rail when its neighbour changes, so a rail whose
answer was refused stays as it is. A falling block never crosses: its fall is one
column.

The runner deduplicates effects among those not yet sent (its queue for the next
commit): one update per position, direction and kind (the latest), one scheduled tick
per position and block (the earlier, as `scheduleTick` keeps it), one spread per
position and direction. A commit's effects are split over records within the log's
`MAX_RECORD_LENGTH`.

### 5. Use, and effects in

**Use.** The store keeps which region uses which chunk:

- Use begins in the group in which the commit thread passes a region's load on or
  grants its claim.
- It moves with the holding: the record of a merge makes the absorbed region's chunks
  in use by the absorber, the record of a split makes the part's chunks in use by the
  part, untaken effects kept with them. The answer to the merge or split names the
  chunks whose use moved.
- It ends at a return (from the request; a chunk being returned is not in use), at a
  new request **`Dropped`**, which the runner sends whenever it stops having a held
  chunk loaded or warm, for whatever reason (its tickets went and it is not yet
  returned, a warm chunk not taken again within `WARM_FOR`, among them those a merge,
  split or restore listed, pinned land it lets go), and at an answer
  `Unreadable` or `Withdrawn` to the load or claim.
- A claim that calls off a return of the same chunk begins its use again.
- A hello of a region to the store (a restore after a crash, a move, a restart of the
  store, after which every runner restores) keeps the use of its chunks: `Restored`
  lists them. The resend after a hello delivers their effects as they are loaded
  again. Use is not a record of the log: after a restart of the store every chunk a
  region holds by the table is in use by it until its `Dropped`, which is more than was
  in use and so loses nothing.
- **Use timers.** For every chunk listed (by the answer to a merge or a split, or by
  `Restored`) that the runner neither has loaded nor holds warm in memory, it keeps a
  use timer, and sends `Dropped` for the chunk if it has not loaded it within
  `WARM_FOR` ticks of the tick it began from. A chunk listed and in memory is warm, as
  warm chunks are now.

**Filing.** A group's records are applied in log order: use and couplings change at
their record, and an effect is filed and converted against the state at its own record.
When a group is durable, the commit thread files each of its effects under its target
chunk so: an effect whose chunk is in use goes on that region's stream; one whose chunk
nobody uses is dropped, as a record. Effects still untaken when a chunk's use ends are
dropped the same way.

**In the worker.** The worker keeps what arrives until a tick can use it: effects for a
chunk it has claimed or asked to load and not yet loaded, or that is warm and not yet
taken again, copy updates until their base. An effect is **ready** when its target
chunk is loaded, every copy in its target's 3x3 is based, and the stream's mark has
reached the effect's sequence. A copy counted absent (`Unreadable`) is not waited
for. At most `MAX_EFFECTS_PER_TICK` (4,096) ready effects go to one tick, the rest to
the next.

**In the tick.** Bases and copy updates are applied with the chunks, in
`update_chunks`, before `update_ticking`, so that ticking counts them. Ready effects
run where seam entries run in ADR-0024's order (after the inputs and `update_ticking`):
by target chunk, in each the chunk's seam entries first, in their order, then the
effects by number. An update whose target's six neighbours are not all loaded or
copied is kept as a **seam entry in its target chunk**; a scheduled tick, a spread and
a conditional set need only their target chunk and run at once.

**Taken as a set.** The holder's commit names the numbers of the effects its tick took
(ran, or kept as seam entries). A tick that takes effects commits even if nothing else
changed. The store drops what is taken. After a hello, a merge or a split it resends
the untaken effects of chunks in use on the stream; the worker drops those it has
taken.

A warm chunk, and a chunk with a use timer running, counts as loaded for `Dropped`:
beginning anew after a merge drops none. So no chunk stays in use that its region will
never load.

**Durable in the log.** An effect is a record of the commit that made it; a taking or
a dropping is a record of the group that did it. Collection keeps a segment while it
holds an untaken effect. When collection would remove such a segment, the store first
writes its untaken effects forward, as records of a group of their own, synced, and
only then removes it; a failure of that group cuts it off like any other. Records keep
their numbers. A make-over for another division re-files the untaken effects of the
commits it passes over before writing its `Opened` records. A group cut off takes its
effects, takings and droppings with it, undone in memory as stays are.

### 6. Seam entries

A seam entry (ADR-0024, section 5; `SeamKind::Shape` or `Neighbour`) is now kept for
an update a region's own tick could not run because a neighbour's chunk was not loaded
or copied, where the target is in land the region holds or has asked for (in the
chunk the update came from, as now), and for a received update that cannot run yet (in
its target chunk, section 5). Both run once the target's six neighbours are readable;
the format does not change. An update into a chunk that is `Foreign`, or `Unknown` and
not wanted, is an effect. A seam entry whose target is `Foreign` is turned into an
effect when its chunk is loaded, and when the target comes to be `Foreign` while its
chunk is loaded (a claim answered `Foreign`, a split, a return of the target's chunk):
a player walking to a border makes claims all the time, and the updates kept while one
was out must not wait for good.

### 7. What changes where

- `clustine-sim`: `Knowledge` gains a border copy; one read path (section 4); effects
  out; bases, copy updates and effects in, with readiness decided by the runner;
  received updates kept as seam entries; the border time of section 8 and scheduling
  by it; `update_ticking` and post-processing count copies; restore and its tests take
  copies.
- `clustine-rpc`: `Commit` gains effects and takings; the stream's messages (effects,
  copy bases, copy updates, partners' progress, marks); requests for copies, `Dropped`,
  copies beside a ticking chunk, the handshake's "couple with" and "joins at", the
  coupling, its end and withdrawn copies on the stream, and couplings in `Restored`
  (section 8).
- `services/worldstore`: sequences; use, moved by merges and splits; effects filed by
  use and dropped otherwise; copies listed on the commit thread and based on the thread
  for chunks; the stream; couplings, the handshake, offsets, the conversion of
  effects, the regions given up, and the file `regions/couplings` (section 8); the
  retention reason,
  writing forward, the make-over's re-filing, the undo of a cut group.
- `services/worker`: the copies its viewers' and guests' 3x3 needs, kept across
  beginning anew; keeping what arrives until it is ready; `Dropped`; commits for
  takings and, while coupled, every tick; waiting for partners' progress; the
  handshake; handing a copy's base to the sim only once its regions are coupled, and
  withdrawing it.
- ADR-0024: section 5 is replaced by sections 4 to 6 here; section 6's first snapshot
  waits for a copy on its way as for a neighbour being loaded; section 6a runs effects
  with seam entries; section 4 counts chunks that tick beside copies among those with
  ticks pending. The edge: nothing.

### 8. Lockstep while a border ticks

Asked by the owner on 2026-10-16, so that a scheduled tick that crosses a line runs on
the tick it would run on in one region.

**Who couples.** The store, which alone knows both which copies each region has and
which region uses each copied chunk. A runner tells the store, in its lane, when a copy
comes to be **beside a chunk that would tick with its copies handed in** (a viewer's
or guest's chunk whose 3x3 is all loaded or based copies), and when it stops being so.
Two regions **need coupling** while one has such a copy of a chunk the other uses.

Each copy's base, and each later change of its chunk's use, says on the stream whether
the chunk is used, and by a region coupled to this one. The runner hands a copy's base
to the sim (`update_chunks`) only once its chunk is used by nobody or by a coupled
region, and **withdraws** a copy whose chunk comes to be used by a region not coupled to
this one (a claim of land nobody held; a return and another's claim) until the two are
coupled. Until then the chunk beside it does not tick. So nothing crosses a ticking
line uncoupled, except where a coupling is given up or ending (below). A region in
`Preparing`, whose couplings have ended, keeps its copies handed in until its stop;
what crosses its lines then is late (section 1).

**The handshake.**

1. The store puts **"couple with P"** on both regions' streams, in one group, and
   records the handshake as under way in `regions/couplings`. It sends none to a region
   in `Preparing`, being moved, or without an owner.
2. Each runner, taking it, names its **next tick not yet run**, commits a record "joins
   at tick n", and does not run tick n until the handshake ends.
3. When both records are durable, the store writes the **coupling**: offsets such that
   both named ticks have the same **border time** (a region's tick plus its offset). A
   region with no coupling takes its partner's border time; where both are in groups
   of coupled regions already, the smaller group's offsets all shift by one constant,
   as a record in sequence order. Where both are in the same group already (the
   handshake closes a ring), offsets stay as they are; a partner may then be up to the
   ring's length ahead, and the first effects that cross run at the taking tick, late
   as section 1 says. Border time is a label, so a shift moves nothing back, and offsets
   stay consistent around a ring. The coupling goes on both streams; both run their
   named ticks.
4. The store **calls a handshake off**, as a record on both streams that ends the hold,
   at a hello of either region, a `Preparing` or a move of either, a giving up of
   either, or when one side has not made its "joins at" durable within
   `COUPLING_PATIENCE` of the group that said "couple with"; that side is then given up,
   and must earn its coupling back. A handshake is not called off because the need
   ended: it completes, and `COUPLING_GRACE` ends the coupling, so that need coming and
   going at a view's edge does not make handshakes flap.

A region held in a handshake counts as waiting. A region may be in several handshakes
at once; it names the same next tick to each, and a call-off ends only that
handshake's hold.

The pause is a commit and a delivery each way. Couplings, offsets, handshakes under way
and the regions given up are kept in a file of their own, rewritten as the table is
(`regions/couplings`), and `Restored` carries a region's couplings, offsets, partners'
progress and any handshake under way.

**The rule.** A coupled region runs its tick at border time t only once its stream has
said that each partner is durable through border time t - 1 (partners' progress is
given in border time). The store puts a partner's effects on the stream before that
partner's progress: they are records of the same commit, in the same group. A coupled
region commits every tick, also when nothing changed. A tick held for a partner is tried
again as progress arrives (every `COMMIT_POLL`, a millisecond) and keeps its deadline:
a held tick does not lose its 50 ms slot.

**Exact delays.** An effect carries the tick it was made in, its maker's own number.
When its group is durable the store converts it, in log order, by the offsets in force
in that group: if its maker is coupled to the holder and the tick is at or after the
tick the maker joined at, the effect is **coupled**, and a scheduled tick made at the
maker's tick m with delay d is due at the holder's tick m + offset(maker) - offset(holder)
+ d. Otherwise the delay counts from the tick that takes it. A scheduled tick whose due
tick has passed when it is taken is due at the taking tick. With the rule above a
coupled effect is taken at the border time after its making whenever it is ready then
(section 5), so water (5 ticks a block), lava (30, or 10) and sand (2) cross a coupled
line on the tick they would in one region; an update, which the game runs within the
same tick, runs exactly one tick late. Ticks due on the same tick order as the holder
schedules them, so a crossing tick runs after the holder's own ticks of the same
priority made a tick earlier, which the game would interleave by when each was
scheduled.

**Ending.** The store ends a coupling, as a record on both streams, once neither region
has needed it for `COUPLING_GRACE` (a second, so that a view's edge going back and forth
does not make it flap). Effects filed after that record convert as uncoupled. The record withdraws, on both
sides, the copies of chunks the former partner uses; no chunk beside them ticks then,
since neither region needs the coupling. So couplings last while a border ticks, and do
not gather into chains across the world. A coupling ended by a giving up or a lost
owner leaves the copies handed in.

**Never stuck.** A region waits only for its partners' previous border time, so the
region furthest behind can always tick: no line or ring of coupled regions waits for
itself. A region is **stalled** when its progress has not moved for
`COUPLING_PATIENCE` (a quarter of a second, measured in E7) counted from the group in
which the last of its partners became durable through its next border time less one,
that is, from when it stopped waiting. A region that waits counts nothing; of two
healthy but slow regions coupled to each other, the one behind is ticking and the
other waiting, so neither looks stalled. So is a region whose border time advances at under 15 ticks a second for five seconds
while a partner waits on it (the owner, 2026-10-16): a region overloaded but still
moving would otherwise hold its partners to its pace for as long as their border ticks.
Ticks a second are measured by the store's own clock over the commits that arrive, and
counted afresh from a hello. The store **gives up** a stalled region: it
ends every coupling of that region, as a record, and marks it given up; the regions
that waited for it tick on. Those coupled to them are not touched, so a stall ends one
region's couplings, not a chain. A region given up that still needs coupling commits
every tick, so that its pace can be seen, and is coupled again by the handshake once it
has committed at 15 ticks a second or more for five seconds; until then what crosses
its lines is late (section 1). Copies already handed in on either side of its lines
stay handed in while it is given up, also where it comes to use a chunk another region
copies: the giving up wins over the withdrawing of section 8's first paragraph. Effects
already filed keep their conversion.

**Reshapes, moves and restores.** A region entering `Preparing` for a merge or a split,
or released for a move, asks the store, in its lane, to end its couplings. The store
also ends a region's couplings at once when its handle is closed, when another owner's
hello replaces it, or when the log fails; a worker that is hung or cut off but keeps its
connection is found by patience. A restart of the store keeps couplings, from
`regions/couplings`. Its
partners wait for it at most until that record reaches them, and it ticks on, still
waiting for them, until its stop. A merge's survivor, a split's parts and a region
restored after a move or a crash are coupled again by the handshake once they tick,
without earning it as a region given up must.

**What it costs.** A coupled pair ticks at the slower one's pace, plus a commit and its
delivery each tick: with the cache a fraction of a millisecond. On the disk without it
a commit can wait for one sync and go through the next, about 44 ms with the delivery
and the tick still to come, so a coupled pair there is likely to run below 20 ticks a
second. "An overloaded region slows down alone" (architecture.md) no longer holds along
a ticking border: a slow region slows the regions coupled to it, until it is given up.
Coupled regions commit every tick, about 6,000 commits between two checkpoints of a
region, all replayed at a restore. E7 measures all of it, on the disk without its cache
too if the owner's machine can be measured so.

## Consequences

- A border is no longer a seam a player meets: water crosses it at its own pace, a
  torch whose support is broken across it falls a tick later, leaves across it decay,
  the chunks along it tick. The half minute after a hand-over is the same.
- Along a ticking border, regions tick in lockstep, and only while it ticks (a second's
  grace); one that stops holds its partners back for up to `COUPLING_PATIENCE`, and one
  that runs under 15 ticks a second for up to five seconds, before they go on without
  it.
- Redstone, built later on the same machinery, settles across a line, and a circuit
  that counts ticks keeps its timing across a coupled line except for updates, each a
  tick late where it crosses (the owner, question 2).
- The store routes effects, makes copies' bases and keeps couplings: more work on its
  commit thread (filing, listing, the stream) and on its thread for chunks (bases);
  coupled regions commit every tick. Measured in E7.
- More chunks tick with ticks pending (those beside copies), so more are saved at a
  stop for a merge or a split (B8 measured 20 to 40 a stop).
- A chunk that stops ticking when its copy goes writes its ticks into itself and takes
  them back, later, ahead of newer ones, as any chunk that stops and starts.
- [ADR-0006](0006-static-regions-and-handoff.md)'s rejection of copies of neighbours'
  border chunks is reversed for chunks beside viewers' and guests' chunks, and only of
  committed forms. ADR-0003's rejection of synchronised ticks stands for the world;
  lockstep is between neighbours whose border ticks, and gives way to a partner that
  stops.

## Steps

As the plan's section 5: E1 this record, reviewed; E2 the shared types; E3 the store,
with couplings; E4 the sim; E5 the worker, with lockstep; E6 tests from this record by
someone who did not write the code; E7 measurements on the real disk, among them a
coupled pair's pace on the disk without its cache if the owner's machine can be
measured so, `COUPLING_PATIENCE`, and the store's load along a busy border. E3 and E5
are not delegated.

## Found while building

E2, the shared types (2026-10-16):

- **A sequence is a record's place in the log, not a group's.** The log marks no group
  boundaries, so a group's position could not be found again after a restart. An
  effect's sequence is the place of the record that carries it (its segment and its
  offset in it, increasing), and its number that and its index in the record; a mark
  says the place of a group's last record. Section 2's order is unchanged by this.
- **An effect is converted, or left out, as its commit is appended.** Records are
  applied in log order, so the use and the offsets "in force at its record" are those
  of the commit thread when it appends the record. The commit's record keeps each
  effect with when it is due (log kind 10), so that a restart files it again without
  the offsets of that time; an effect whose chunk nobody uses then is not written at
  all. Effects dropped later (a chunk's use ending) are a record of their own (kind 11),
  since use is not in the log and a restart would otherwise take them for kept;
  effects written forward are kind 12, with their numbers and when they are due.
- **A coupled effect's due tick counts in its holder's ticks.** Where the use of its
  chunk moves to another region before it is taken (a merge, a split, a return and
  another's claim), it counts from its taking instead.
- **Couplings are kept in their file, not in log records.** `regions/couplings` (the
  region's offset, couplings, handshakes under way, regions given up; format in
  `clustine-format`, kind 5 of the store's files) is rewritten in the group that
  changes it, as the table is; since effects are written already converted, nothing
  replayed needs the couplings of an earlier time.
- `Restored` carries the chunks in use as one more item of its parts, behind the
  grants; the couplings, handshakes and whether the region is given up travel in the
  welcome.

## Review

**First review of this record (2026-10-16): not ready.** Its findings, worked in:

1. Use across merges and splits was undefined, so effects at the old line were
   dropped at the merge that used to run them: use moves with the holding (section 5).
2. Use began at a load's answer, dropping effects filed while chunks were loaded or
   restored: it begins when the load is passed on; crashes, hellos, moves and store
   restarts keep it (section 5).
3. A copy's sequence was undefined, so an effect could wait for ever on a copy that
   never changes: one stream per region with marks; a copy is current through the last
   mark; updates name their chunk's previous change (sections 2 and 3).
4. A base that never comes: `Unreadable` for copies, counted absent (section 3).
5. Ring tickets contradicted ADR-0024: copies only beside viewers' and guests' chunks,
   only of `Foreign` or unwanted `Unknown` chunks, dropped when loaded (section 3).
6. Received effects did not fit seam entries: only updates are kept so, in their
   target chunk, with the format unchanged; ticks, spreads and sets run at once
   (sections 5 and 6).
7. Copies were dropped at every merge of the copying region: they survive beginning
   anew; the absorber asks for those along its new borders (section 3).
8. A region's own updates into land it holds or asked for stay seam entries (sections
   1 and 6).
9. Stored seam entries whose target is `Foreign` become effects when read (section 6).
10. Bases and copy updates are applied in `update_chunks`; seam entries before effects
    in a chunk; the runner deduplicates (sections 4 and 5).
11. Effects are numbered by sequence and index (section 2).
12. The direct lookups to go through the one read path are named (section 4).
13. Wording of copies of returned land, and copy updates without ticks or marks
    (section 3).
14. Numbers named: records split within `MAX_RECORD_LENGTH`, `MAX_EFFECTS_PER_TICK`,
    when writing forward happens (sections 4 and 5).

**Second review of this record (2026-10-16), focused on sections 2, 3, 5 and 8: not
ready.** Its findings, worked in:

1. The store could not choose offsets, not knowing the tick a region is on: a
   handshake in which each names and holds its next tick; a tick due in the past runs
   at the taking tick (section 8).
2. Effects carried border times that go stale: they carry their maker's tick, and the
   store converts it at filing by the offsets in force then (sections 4 and 8).
3. A chunk could come to tick beside a region without a beginning anyone saw: the
   store couples, from copies beside ticking chunks and use; a base is handed to the
   sim only once its regions are coupled, and withdrawn when its chunk gains a user not
   coupled (section 8).
4. Patience dissolved chains and made slow partners flap: only a region that waits for
   nobody is given up, and coupled again only after keeping pace (section 8).
5. Use outlived a crash or a move: it ends at every hello and begins again with loads,
   the first hello of a split's part excepted (section 5).
6. `Dropped` whenever a held chunk stops being loaded (section 5).
7. Seam entries stranded when their target came to be `Foreign`: turned into effects
   then (section 6).
8. A spread into a chunk with no copy of its source was never ready: it carries the
   source and needs no copy (section 4).
9. A claim that calls off a return begins use again; the worker keeps effects for
   chunks claimed and not yet loaded (section 5).
10. Reshapes and writing forward take sequences; bases follow `remakes`; a store
    restart rebases copies (sections 2 and 3).
11. Where couplings live (`regions/couplings`), `Restored`, and how a reshape ends them
    (section 8).
12. The cost of committing every tick, and the order of ties, named (sections 1 and 8).

**Third review of this record (2026-10-16), focused on sections 5 and 8: not ready.**
It found the exact delays, the keeping of couplings over a restore and the conversion
of effects made before a coupling sound. Its findings, worked in:

1. A handshake could not be called off, and a crash, a `Preparing` or a store restart
   in the middle of one held a region for good: handshakes under way are kept and
   carried in `Restored`, and called off at a hello, `Preparing`, a move, a giving up
   or the need ending; none is begun with a region that cannot take part (section 8).
2. "Need coupling" was circular: a copy beside a chunk that would tick with its copies
   handed in (section 8).
3. Couplings never ended and would gather into chains across the world: they end a
   second after neither region needs them (section 8, "Ending").
4. Patience counted from the wrong moment, and a region given up could never earn its
   coupling back: counted from when it stopped waiting; a region given up that needs
   coupling commits every tick (section 8).
5. Use leaked at a merge: the answer names the chunks whose use moved, counted warm,
   dropped after `WARM_FOR` (section 5).
6. Use ending at every hello lost effects at routine moves: use is kept across a
   hello, its chunks counted warm; couplings end at a move's release and at a lost
   owner at once, and are made again after a restore without earning (sections 5 and
   8).
7. A handshake closing a ring: offsets stay, the first effects run late; progress is in
   border time; a shift is a record (section 8).
8. A held tick lost its slot: retried as progress arrives, its deadline kept; the cost
   without the cache restated (section 8).
9. Warm chunks count as loaded for `Dropped`; a base says whether its chunk is in use
   and by whom; a region in `Preparing` keeps its copies; the named tick is the next not
   yet run (sections 5 and 8).

**Fourth review of this record (2026-10-16), focused on sections 5 and 8: accept after
named changes, no further review needed.** It found the exact delays, the shifts of
offsets, held ticks keeping their deadlines and the keeping of couplings over a restart
of the store sound. Its changes, made:

1. Chunks listed by a merge, a split or `Restored` that are not in memory get a use
   timer, not a pretence of being warm (section 5).
2. Use is not a log record, so after a restart of the store every held chunk is in use
   until `Dropped` (section 5).
3. A coupling that ends for want of need withdraws the copies of the former partner's
   chunks; a giving up leaves them, and wins over a withdrawing (section 8).
4. A handshake has a limit of its own, and a region held in one counts as waiting;
   several handshakes at once (section 8).
5. A region running under 15 ticks a second for five seconds while a partner waits is
   given up, as the owner chose (section 8; Consequences).
6. A lost owner is a closed handle, a replacing hello or a failed log; a restart of the
   store keeps couplings (section 8).
7. A handshake is not called off because the need ended (section 8).
8. Records in a group apply in log order (section 5).
9. Ticks a second are measured by the store's clock over arriving commits, afresh from
   a hello (section 8).
