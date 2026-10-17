# Phase R: redstone and pistons across a line, the ways open

Draft of 2026-10-17, for the owner's answers of the same day to the [plan of phase
R](r-plan.md)'s questions 2 and 3: "look for more" (a way to make redstone across a
coupled line closer to the game) and "carry moves across" (a piston's move may cross
a line). To be reviewed independently, then put to the owner.

## 1. What is wrong at a line today

Two regions whose border ticks run in lockstep (ADR-0025, section 8): each runs tick
N once its partner is durable through N − 1, and reads the other side from copies of
its committed state. What a tick does across the line is an effect, filed with the
commit and taken by the other side at N + 1. So:

- an update crosses a tick late; a cascade that crosses k times takes k ticks;
- a scheduled tick made by an effect counts from the maker's tick, so it is on time;
- within one tick, each side sees the other as it was at the end of the last.

In the game a tick is one thread: an update and everything it sets off run at once,
depth first. Redstone lives in that: dust turns a line off within a tick, a door and a
lamp react in the tick of their lever, a flip-flop's two paths meet in the same tick.
A piston's move is one block event that sets, in one go, every block it moves.

## 2. Where the line's cost comes from

Nothing in the record makes a crossing late but the order of three things: a region
runs its whole tick, then commits, then the other side reads it. Any way to make a
crossing on time either (a) lets the two sides talk inside a tick, or (b) puts the
circuit into one region, so that it does not cross.

## 3. The ways

### A. Exchange inside a tick: the pair's tick in phases

The two coupled regions run each of a tick's phases (block ticks, fluid ticks, random
ticks, block events, block entities; the seam phase and inputs before them) and then
meet at a **barrier** before the next phase: each hands the other what its phase did
across the line (updates, sets) and the blocks it changed beside the line, and runs
what it was handed, in rounds, until neither has anything more for the other or a
bound of rounds is reached (what is left goes as now, a tick late). Copies are brought
up to date at each barrier, so a phase reads the other side as it is.

- **What it gives**: dust, doors, lamps, rails across the line in the tick they
  would be in the game; skew between two paths gone wherever both arrive within the
  phase. Within a phase the order across the line is by rounds, not depth first: a
  contraption whose result depends on the order of two updates on opposite sides
  can still differ, far more rarely than a skew of a tick.
- **Pistons**: a move that crosses is proposed at the block-event phase's barrier,
  the other side says whether its blocks are as the mover read them, and both apply
  it in the same round; refused, nothing moves, and the piston tries at the next
  update as the game would at a blocked move. The moving piston's three ticks run on
  each side for its own positions.
- **Durability**: a tick of one region now depends on what its partner did in the
  same tick. If the partner crashes before its tick N is durable, its restore may
  not do N as it did (its players' inputs are not replayed to the same ticks), so the
  store **commits a coupled pair's tick N together**: it holds each side's commit of
  N until the other's has come and writes both in one group. A partner lost before
  that ends with its partners restored from N − 1 as well: one region's crash
  restarts the regions coupled to it, which lose at most the ticks not yet durable.
- **Cost**: a round trip per round at each barrier, five barriers a tick while
  coupled, a fraction of a millisecond in one machine and about a network round trip
  between machines, so a few milliseconds of a tick's 50 at worst; a tick that waits
  at a barrier for a slow partner. Patience and giving up (ADR-0025, section 8) apply
  to barriers too. The simulation's tick is cut into phases with inputs between them;
  it stays a function of the region and its inputs.
- **Size**: the largest of the ways: the simulation's tick, the worker's runner, the
  store's commit of pairs, the stream. A record of its own, ADR-0025's successor at
  the line.

### B. Lend the circuit's land to one region

When something that reads or carries power crosses a line, the chunks of the other
side that it touches are **lent** to the region it started in, a band one chunk deep
along the line, grown as the circuit grows up to a bound. The lender keeps a copy of
each lent chunk; its players see it and act on it as on any land of another region (a
tick or two late, ADR-0012), and one who walks into a lent chunk is handed over as
across any line. When the circuit has been quiet a while (half a minute, as a region
empties), the land goes back.

- **What it gives**: a circuit that fits in the band runs in one region, exactly as
  in the game, timing, order and pistons included. A circuit larger than the bound
  still crosses, at the band's far edge, as now.
- **Durability**: a lend is a change of holder in the store's log, as a grant is;
  nothing about a tick depends on another region's same tick. A crash restarts only
  the region that crashed.
- **Cost**: there is no hand-over of a chunk in use today (a chunk changes holder by
  being given back unused and claimed, by a merge or by a split). A lend needs one:
  the lender stops ticking the chunk, saves it, turns it into a copy and lets it go,
  and the borrower loads it, in a few ticks; meanwhile the crossing is as now, so the
  first time a circuit fires across a line it is a tick or a few late, and from then
  on on time. A player standing in a chunk to be lent is handed over with it or the
  lend waits. At a pinned line the pin moves, locally and for a while: the land a
  region is pinned to is no longer all it holds.
- **Size**: a new kind of hand-over in the store and the coordinator, the lend's
  trigger in the simulation and its return. Smaller than A, and it reuses claims,
  copies and hand-overs of players.

### C. Keep lines off circuits

For regions that follow players, the coordinator would not draw a split's line
through land where redstone has been active lately, and would merge two coupled
regions with redstone active across their line. At a pinned line nothing changes.

- **What it gives**: between regions that follow players, circuits stay in one region
  nearly always; pinned lines as the plan has them.
- **Cost**: small; the coordinator's choice of lines and merges, told of activity by
  the regions.

### D. As the plan has it

A tick late per crossing, settled results that can differ, pistons that do not cross.

## 4. Pistons that carry their moves across, alone

Without A or B, a move across a line could be carried as effects: the mover sets its
own side and files conditional sets of the other side's blocks (each to happen only if
the block is as the mover read it from the copy). The other side takes them a tick
late; if one fails, because that side changed the block in the same tick, the mover's
side has already moved and the other has not: a block duplicated or lost. Making it
safe needs a proposal answered before the move, two ticks or more of delay on every
crossing move, and the moving piston's three ticks out of step on the two sides. Of
the four, it is the worst: A and B both carry moves across as part of what they do.

## 5. Where a line ticks, and what I recommend

**Lines between regions that follow players almost never tick.** Regions merge while
their players are still 22 chunks apart, four chunks between their lands, and a split
draws its line between groups more than 30 apart, beyond everyone's view
(ADR-0017, sections 3.5 and 3.6). A chunk nobody sees does not tick, so no copy is
beside a chunk that would tick and no coupling is made (ADR-0025, section 8): such a
line ticks only for the seconds a wanted merge waits, or while someone looks into a
trail as a guest. What the owner runs (`cargo run --release -p clustine`) meets the
gaps of section 1 hardly at all. **The case is the pinned line**, which the README
offers as "a boundary at a known place" across which players "build across the
boundary, without noticing it".

So:

- **Between regions that follow players: merge at once.** When two coupled regions
  have redstone active at their line, the coordinator merges them without waiting for
  the merge's usual rest. C reduced to that; cheap.
- **At a pinned line: B, lending at build time.** When a player places a component
  whose reach crosses a pinned line (two blocks for most, the 13 of a piston's
  structure for a piston), the chunks of the other side within that reach are lent to
  the builder's region (only for a component placed in the region's own pinned land:
  one placed in lent land lends nothing more, so lends do not creep across), and stay lent while any component reaches over; not on firing
  and not given back after a quiet half minute, so a door used every few minutes pays
  nothing when used, and nothing stalls when it fires. The builder is the region the
  player is in, so who decides is the region that places, through the store, and two
  builders on the two sides reaching over the same chunk are told apart by the first
  lend granted; the second's component waits (refused, as a placement into land not
  held is). The pinned region's players see lent land and act on it as on any land of
  another region, a tick or two late; one who walks into it is handed over. The table
  already lets a grant override a pin and gives the chunk back to the pinned region at
  a release, so a lend of pinned land is a grant with a reason; a lend of land held by
  grant would need a new record, and is not needed while lends are only of pinned land.
- **Not A.** Its "same tick" is not the game's either: reads within a phase see the
  other side as at the last barrier, so two diodes facing each other across the line
  due on one tick run in the wrong order, and dust's cascade across it needs up to 15
  rounds a crossing; its commits must cover whole coupled groups, so a crash restarts
  everything transitively coupled (with `--pin 0,4`, the world), and giving up a
  partner mid-tick is a restore; it needs a channel between workers that does not
  exist; and every coupled pair pays its eight barriers a tick, water or not.
- **Not section 4's moves alone.**

## 6. The review (2026-10-17), and what changed

1. The note aimed at lines between regions that follow players, which almost never
   tick; the pinned line is the case (section 5).
2. A's "same tick" was overstated: reads within a phase are stale, rounds stand in for
   depth first, two piston queues refuse moves the game makes; eight barriers, not
   five (section 5).
3. A's commits must cover whole coupled groups, and giving up mid-tick is a restore;
   no channel exists for barriers (section 5).
4. A's cost is paid by every coupled pair (section 5).
5. B's "exactly as the game" needs everything a circuit's updates reach inside held
   land; the lender's players' clicks are a tick or two late; lending on firing
   stalled the first use after every quiet half minute, and the builder standing in
   the chunk blocked it; nobody decided; the table has no lend between grants: B now
   lends at build time, of pinned land only, decided by the placing region (section
   5).
6. Section 4 stands; neither A nor B carries every move across.
7. C reduced to merging at once.
