# ADR-0027: Lending land at a pinned line, and merging at once at a busy one

- Status: Accepted on 2026-10-17, from the owner's answer to the ways of carrying
  redstone across a line ([r-lines.md](../groundwork/r-lines.md), section 5, reviewed:
  "lending at build time"), and ADR-0026's section 10. Three reviews found it not ready and a
  fourth ready after small fixes; their findings are worked in ("Review").
- Date: 2026-10-17

## Context

Redstone inside one region behaves as in the game (ADR-0026). Across a line between two
regions it does not: what crosses is an effect, a tick late while the two are coupled
(ADR-0025), so dust goes dark slowly across a line, two paths that cross it differently
can make a pulse the game would not, and a piston whose move would reach across does not
move at all (ADR-0026, section 10).

Lines between regions that follow players almost never tick: regions merge while four
chunks still lie between their lands, and a split draws its line beyond everyone's view
(ADR-0017, sections 3.5 and 3.6). The case a player meets is the **pinned line**, which
the README offers as a boundary at a known place that players build across without
noticing. The owner chose on 2026-10-17 that land there be **lent at build time** to the
region a circuit is built in, so that the circuit runs in one region, kept while the
circuit stands and not given back after a quiet half minute; and that regions that
follow players with redstone active at their line be **merged at once**.

What the store, the table and the simulation do today (from a read of the code,
2026-10-17):

- A chunk is held by its grant, else by the region pinned to the area it is in
  (`Table::holder`). A grant overrides a pin; giving the grant back
  (`Table::release`) leaves the chunk to the pinned region again, and nobody tells it.
- A claim of a chunk another region holds is answered `foreign`. A pinned region never
  gives its pinned land back (`Land::keeps`), and its `Return` of a pinned chunk only
  ends its use, without waiting for any save. A pinned region doubts a `Foreign` belief
  in its own area and drops it at an arrival or a remote action.
- A region's use of a chunk (ADR-0025, section 5) begins at a load or a grant and ends at
  a return, at `Dropped` or at `Unreadable`, which drops the effects filed for the chunk;
  a merge or a split moves use, with the untaken effects (`Uses::moved`).
- A region never lets go of a chunk that has tickets, so nothing tells an edge that a
  chunk it is served is moving to another region.
- A player standing in a chunk the region believes another's is let go to it at the end
  of the tick (ADR-0016); a player of one region acts on another's blocks through the
  edge, a tick or two late (ADR-0012, sections 2.3 and 2.4).
- Pinned regions are merged and split by distance unless the server runs with
  `--reshape by-hand`; a split's part is not pinned, and holds its parent's pinned
  chunks by grant.

## Decision

### 1. What a lend is

A **lend** is a grant of a chunk of another living region's pinned area, the **lender**,
to a pinned region beside it, the **borrower**. Nothing in the table is new: the grant is
durable as every grant is, carried by the borrower's merges and splits, and ended by the
borrower giving the chunk back, after which the chunk is the lender's again. A grant
inside no living region's pinned area (an open world's, a split part's of its parent's
pinned land) is no lend. A line between a pinned region and a split part of its
neighbour, or of its own, has neither lending nor merging at once; a line between
regions that follow players has merging at once (section 7).

The store tells each region which of its chunks are lent to it and which of its pinned
chunks are lent away, with what it is restored with (`Holdings::lent` and `Holdings::lent_away`, worked out from the table and carried in the store's answers to a hello and a merge
(`Restored::lends`, `StoreReply::Absorbed::lends`), since the survivor of a merge forgets
what it believed of others; at a split the region works out its own, as it chooses what
goes), so that both survive a restore. And it tells a pinned region whenever any grant inside its
area ends (`StoreReply::PinnedBack { chunks }`), whoever had it: a lend given back, a
lent chunk that went to a split part and was given back by it, or a part's own grant.

**One way per stretch of a line.** The areas are stripes along x, so a line runs the whole
length of z. Between two regions, lends go one way at a time within a stretch of eight
chunk rows of z (z divided by eight): the first lend granted between a pair in a stretch
fixes who borrows there, until the last lend between them in that stretch ends; a request the other way there is not lendable. A lend under way counts as granted for
this, so two asks crossing in one stretch cannot both be granted. So land never swaps across one line, does not
zig-zag from one row to the next, and two circuits far apart along one line may be built
from either side. The stretch's way is the table's lends and the store's lends under way, so it needs no
durable state of its own.

### 2. When a lend is asked for

What asks is a **circuit's part**: wire, a torch, a repeater, a comparator, an observer, a
piston, a lever, a button, a plate, a redstone block, a target, a note block, a copper
bulb, a lamp, and any state the block data says is a signal source
(`StateProps::is_signal_source`). Doors, trapdoors, gates, rails and the rest of what
reads a signal do not ask: one of them at a line only opens a tick late.

A region asks for a lend (`StoreRequest::Lend { chunks }`) at **build time** only, as the
owner chose: a circuit's part is placed in the region's **own pinned land**, by a player
of the region or by another region's player through the edge (a remote action), within
its **reach** of a chunk the region does not hold: two blocks for every part, and for a
piston its structure as the resolver finds it at placing, with what slime and honey take
sideways. The chunks across within that reach are asked for. A part placed in lent land
asks for nothing, and nothing but a placement asks: lends do not creep and do not come
at a circuit's firing. A powered rail's eight rails reach further than two blocks and
still cross.

The region keeps what it asked for **in its state** (`RegionState::lends_asked`), so that a
crash before the grant loses nothing. An ask ends at the store's grant or at **not
lendable**; at **wait** (the lender cannot let go now) and at a **call-off** it is kept and
asked again, at most once in twenty of the region's ticks. A merge's survivor keeps the
asks of both that are still for chunks it does not hold and that are not in its own
pinned area; a split's part, which is not pinned, asks nothing and drops its asks; a
restore drops the asks for chunks the region holds. A `Lend` of a chunk whose lend is
under way is passed over.

A lend under way is marked **under lend** in the store from step 1 of section 3 until its
grant, a call-off or the lender's answer, whichever comes first.

The store answers the borrower `StoreReply::NotLent { chunks, again }`, `again` for
wait. It answers **not lendable** where the asker is not pinned or not beside the area's
region, the chunk is the home chunk, is held by a grant, is in no living region's pinned area or in the asker's own, or the stretch's way is the
other by the table's lends (section 1), or the lender says so (section 3, step 2). Where
only a lend under way the other way stands in the stretch, the store answers **wait**,
since that lend may yet be refused. The region keeps the answer with the chunk until
`PinnedBack`, a lend's grant or end, a merge or a split of its own, and does not ask for
the chunk before.

### 3. How a lend is granted

The lender is using the chunk, ticking it, its players see it, and other regions may
send it effects. So a lend is a hand-over of a chunk in use, made in four steps, each
answered before the next:

1. **The store asks the lender** (`StoreReply::LendAsked { lend, chunks, to }` on its
   stream, `lend` a number the store gives each lend under way, which every later
   message of the lend carries, a message with another number being passed over) and
   marks the chunk **under lend**: from then until step 4 or a call-off it grants
   nobody else a lend or a claim of it. The lender's loads, saves and claims of it are
   taken as ever until the lender has given it back.
2. **The lender decides**, in its next tick:
   - it answers **wait** (`StoreRequest::LendRefused { lend, chunks, again: true }`) where
     the chunk, or a chunk of its own beside it, is not loaded with its stored ticks: it
     loads them under a lend ticket, kept until the next `LendAsked` for the chunk or for
     five seconds, so that it never lends without seeing what is there and the next ask
     finds them loaded. A chunk need not tick for this. Within reach is never further
     than the chunks beside: no reach of section 2 passes thirteen blocks;
   - it answers **not lendable** (`again: false`)
     where a circuit's part of its own lies in its own land outside the chunks asked for
     and within reach of them, so that a lend never moves the line into the lender's
     circuit. Parts inside the chunks asked for do not count, since they go with the lend:
     a builder dragging dust across the line has it on the borrower's side, however their
     clicks fall between the two regions' ticks;
   - it answers **wait** where one of its players stands in the chunk, a block event or a
     moving piston touches it, a load or save of it is still unanswered, or it is the
     chunk a player enters the world in; the borrower asks again later, and a builder standing across the line has
     the lend once they step back;
   - otherwise it marks the chunk **lending** in its land (no ticket loads it, no claim
     asks for it, no load or save of it is made again), stops ticking it, saves it as it
     is with its ticks, kept block events and seam entries, unloads it, keeps a copy of it
     (ADR-0025, section 3), holds its edges' subscriptions of the chunk as waiting (section 5), and gives it back
     for the lend (`StoreRequest::Lending { chunks, lend }`, with the lend's number from
     `LendAsked`), not by an ordinary `Return`. A chunk lending or lent away is never
     taken from a `Loaded` answer.
3. **The store waits for the save**: `Lending` does not end the lender's use there (it
   skips `end_uses`); it goes the way a granted chunk's return goes, behind the saves
   asked before it, and is not done until they are durable. A `Lending` whose lend is no longer under way (called off meanwhile) is passed over and
   answered `LendCalledOff { lend }` to the lender, whether or not it was told before, and
   the chunk stays the lender's with its use and its effects, taken up again at that
   answer.
4. **The store grants it** in the group in which the save is durable: the use of the
   chunk passes from the lender to the borrower in that group, with the effects filed for
   it, as a merge moves use; those the lender was sent and had not taken are sent to the
   borrower behind a mark, their due ticks counted again from the taking, as at a merge,
   since the borrower's ticks are not the lender's, and the chunk's copiers are told, as after a merge's `Uses::moved`, the borrower being
   dropped from the chunk's copiers and the lender, which keeps a copy, added; the grant is written
   (`LogRecord::Granted`, the borrower's region, the borrower's lane's latest tick); the
   borrower is answered (`StoreReply::Claimed { granted }`); and the lender is told
   (`StoreReply::Lent { chunks, to }`), on which it drops the effects it received for the
   chunk and has not run (`Border::drop_effects_of`), believes the chunk the borrower's,
   trusts the belief until `PinnedBack`, and tells its edges (section 5).

**Calls off.** A lend under way is called off at any hello of the lender or the borrower, a
merge or a split of either, and a failed log; at step 4 the store checks again that the
lender still holds the chunk by its pin and the borrower lives, and calls it off if not.
A call-off is told to both (`StoreReply::LendCalledOff { lend, chunks }`): the borrower keeps
its ask, and the lender takes the chunk up again as its tickets want it.

Until the grant the crossing is as it was: effects, and pistons that wait. The chunks
beside the lent chunk stop ticking on both sides for the few ticks until the new copies
are based, and the chunk's scheduled ticks are held over that gap, as any chunk's that
stops and starts ticking.

### 4. How a lend ends

The borrower keeps a lent chunk loaded while it is lent, as it keeps its home chunk,
whether players are near or not: the rule that gives back a chunk unused for half a
minute does not give back a lent one. It gives the chunk back when no circuit's part lies
in the chunk, and none in its own pinned land within reach of it. It looks whenever a
block within reach of a lent chunk changes from a circuit's part to something else, by a
player, a piston or an update, and once its lent chunks are loaded after a restore, a
merge or a split; it reads a chunk's sections by their palettes, so that a section
without a circuit's part in its palette is passed over at once. A chunk given back has
its changes saved before, as every return. Once the release is durable, and not before,
the store tells the lender (`PinnedBack`),
which drops its belief and claims the chunk again as its tickets want it.

A lend ends too where the borrower and the lender are merged: the chunk is the
survivor's. A lent chunk that a split hands to a part is the part's by grant and no lend; the part
gives it back as any chunk it does not use, and the lender is told by `PinnedBack`. Where
a merge or a split moves a lent chunk to another region, the store tells the lender the
new holder (`Lent { chunks, to }` again, in the reshape's group), so that its belief stays
true for its players and its edges.

### 5. The edge

Viewers of a chunk are served by the region that holds it, and guests are served by it
for a player of another region. While a chunk is lending (section 3, step 2), the
lender holds its edges' subscriptions of it as waiting and answers a new one so too; on
`Lent` it tells every edge with a viewer's subscription of the chunk that it is elsewhere
(`Elsewhere(borrower)`, with the subscription's ask) and every edge with a guest's that
it is not its own (`NotMine`), and the edges ask the borrower, which by then holds it and
serves them. On a call-off it serves the subscriptions it held. The borrower, too, holds as waiting a subscription of a chunk from sending `Lend` until
the store's answer (wait, not lendable, a call-off or the grant), rather than answering it
`NotMine`; at an answer other than the grant it answers the held subscriptions `NotMine`,
and holds again at its next ask. `Claimed` and `Lent` go in one group, so the hold lasts
ticks, not the minutes an ask may be kept. The edge keeps its
replica of the chunk meanwhile, so players see no hole, and are sent the borrower's copy
when it comes. The lend's end does the same the other way, the borrower telling on its
return and the lender serving once it has claimed and loaded the chunk again. These are
hand-over steps, built by me with the store's (section 8).

### 6. Players

A player of the lender who walks into a lent chunk is handed over to the borrower as at
any line, and one who acts on a block of it acts through the edge, a tick or two late. A
lend is refused while a player of the lender stands in the chunk (section 3, step 2).

### 7. Merging at once between regions that follow players

A region says, with the players it reports to the coordinator (`PlayersOf`), each chunk in
which a circuit's part changed in the last second beside a copy of another region's chunk,
with that region (`redstone: Vec<(ChunkPos, RegionId)>`, at most sixteen, those nearest
the copies first). The coordinator merges two regions at once, without the merge's usual
rest, as a merge wanted for `Why::Redstone`, where one reports a chunk with the other
beside it, unless either is pinned or holds chunks of a pinned area by grant (section 1).

A split must then not draw its line through the circuit again. After a merge for
`Why::Redstone` the survivor reports, for as long as it keeps reporting, the chunks in
which a circuit's part changed anywhere in its land in the last second, nearest the
chunks it reported before first; the coordinator keeps each reported chunk for five
minutes from its last report, the window rolling with each. A split for `Why::Apart` is
not wanted while a chunk so kept lies within two chunks of a chunk that would fall on the
other side of the split's line. The line is the one the region draws (`Sides::goes` in
`reshape.rs`: a chunk goes where it is nearer one of the players who go than any who stay
or the home chunk), which the coordinator works out from the crowds it has, not
`named`'s squares, which lie far from the line at any split's distance.

### 8. What changes where

- **The store**: `Lend`, `LendRefused`, `LendAsked`, `Lending`, `Lent`, `LendCalledOff`,
  `PinnedBack` and the not lendable answer; `Holdings::lent` and `lent_away`; pending
  lends in memory, called off as section 3 says; others' claims and lends of a chunk
  under lend refused; a pinned chunk's return under a lend done behind its saves, without
  ending its use; use passed with the effects kept and resent; the stretches' ways
  (section 1).
- **The worker**: the lender's runner takes `LendAsked` to the region and the region's
  decision back, and tells its edges' subscriptions; the borrower's sends the region's
  requests and returns.
- **The simulation**: the requests of section 2 and their keeping in the region's state;
  the land's states **lending**, **lent away** and **lent**, from `Holdings` after a restore,
  with the belief trusted; letting go of a chunk; keeping lent chunks loaded; the check of
  section 4; the reports of section 7.
- **The coordinator**: section 7.

## Consequences

- A circuit built across a pinned line runs in one region, as in the game, from the
  first firing after its lend is granted, pistons moving across included; a circuit
  larger than its reach still crosses at the reach's far edge, as before.
- The pinned line moves locally, one way, while circuits stand across it. Players of the
  lender use lent land as another region's, a tick or two late, and see it served by the
  other region without a hole.
- Between regions that follow players, redstone at their line merges them, and no split
  draws the line through it again while it is busy.
- The chunks beside a lent chunk stop ticking for a few ticks as it is handed over.

## Steps

L1 this record, reviewed; L2 the store's lends (the requests and answers, the pending
lends and their calls off, the return behind the save, use passed with its effects, the
stretches' ways, `Holdings` and `PinnedBack`) by me; L3a the simulation's requests and
their keeping, the land's states and their durability (`take_absorbed` taking `lent` and `lent_away` from the store's answer, `take_split`
working them out), letting go, keeping and giving
back, by me; L3b the worker's part and the edge's (section 5), by me, since they are
hand-over steps, with tests between L3a and L3b; L4 the coordinator's merge at once and its splits; L5
tests from this record by someone who did not write the code, a crash at each point of
section 3 and a merge or a split of either region with a lend under way among them; L6
a circuit across a pinned line against the same circuit in one region, and what to try
with a real client.

## Found while building

L2, the store's lends (2026-10-17, by me):

- **A lend needs no new state in the table.** A chunk of a pinned area granted to another
  region is already held by it, and a release makes it the pinned region's again; a lend
  is such a grant made by asking the pinned region first. The lend states are worked out
  from the grants (`Table::lends`). A split part's grants in a pinned area are lent away
  for the region whose area it is, which learns of their end by the same `PinnedBack`,
  but no lend for the part, which is not pinned and gives them back as any other.
- **The lender's `Lending` is the chunk thread's return**, unchanged: the commit thread
  notes its number with the lend, and the return's completion grants the borrower instead
  of freeing. A lend under way is in memory only, called off at any hello, merge or split
  of either region; the numbers of lends are never given twice in a store's life.
- **A split's answer carries no lends**: the region chose what goes, and knows it. The
  hello's and the merge's do.
- A chunk the asker holds already is answered as a claim would be; its own pinned land and
  the home chunk are not lendable, whoever holds them.

L3a and L3b, the region's and the worker's part (2026-10-17, by me):

- **A lender decides at the end of the tick it is asked in**, from the world as that tick
  leaves it, and lets go by dropping the chunk at the start of the next: so the runner
  saves it as it was, ticks and all, before `Lending` goes, as a chunk whose last ticket
  goes is saved before the tick that drops it. A lent chunk given back goes the same way.
- **A lent chunk given back can be wanted still**, by a viewer's ring: the region claimed
  it again in the same tick, and the store's rule that a claim keeps a chunk on its way
  back kept it, as an ordinary grant of another's pinned land. A claim of another living
  region's pinned land on its way back is answered with that region now, and the return
  goes on.
- **A region doubts its belief in its own pinned land** when an action or an arrival names
  it, since land handed to a split part came back unsaid before `PinnedBack`. Land lent
  away is believed against both until it is pinned back.
- The subscriptions a lender served of a chunk it lets go of wait, and are answered by
  what it knows once the store has spoken, as any waiting subscription is; the edge needs
  nothing new for `Elsewhere` and `NotMine`, which it takes as it does at a split.
- The section 7 reports, which the coordinator alone uses, are built with L4.

## Review

**First review (2026-10-17): not ready.** Its findings, worked in:

1. A return ended the lender's use and dropped the effects filed for the chunk, so a
   first firing could be lost: use passes to the borrower with them (section 3, step 4).
2. Nothing stopped the lender claiming the chunk back before the grant: the chunk is
   under lend in the store and lending in the lender's land (steps 1 and 2).
3. A pinned chunk's return did not wait for its save: under a lend it does (step 3).
4. A pending lend could stick, or a grant meet a table that no longer fits: calls off,
   and a check at the grant (section 3).
5. The store could not see the lender's players: the lender decides (step 2).
6. The lender was not told, and pistons and block events at the hand-over were open:
   `Lent`, and the lender refuses while either touches the chunk.
7. Land could swap both ways and zig-zag along a line: one way per line (section 1).
8. Quiet lends were given back after half a minute, and lending at firing had come
   back: lent chunks are kept, and only a placement asks (sections 2 and 4).
9. Lends crept from lent land: only the region's own pinned land asks (section 2).
10. The simulation cannot tell another region's pinned area: the store answers not
    lendable, kept until the land changes, asked at most once in twenty ticks
    (section 2).
11. The definition caught split parts and open worlds: a lend is a grant inside
    another living region's pinned area (section 1).
12. The lender doubted its belief: it trusts it until `LendEnded` (step 4).
13. Levers, buttons, plates and redstone blocks were not in the reach, nor slime's
    sideways reach: they are (section 2).
14. The check of section 4 would scan whole chunks: by palettes (section 4).
15. The edge was missing: section 5.
16. Section 6 named messages that do not exist and would flap with splits: `PlayersOf`,
    `Why::Redstone`, and splits kept off a busy circuit for five minutes (section 7).

**Second review (2026-10-17): not ready.** Its findings, worked in:

1. Refusing the lender's loads and saves would stop it, as a worker gives up a region
   whose store says another holds what it saves: they are taken until it has given the
   chunk back (section 3, step 1).
2. A call-off ended the borrower's ask: `LendCalledOff` keeps it (sections 2 and 3).
3. A builder standing across the line is the lender's player, and their placement is a
   remote action: remote placements ask, and a player standing there is answered wait,
   which keeps the ask (sections 2 and 3).
4. The land's lend states were not durable, and a borrower would give a lent chunk back
   after a restore: `Holdings::lent` and `lent_away` from the store (section 1).
5. A lent chunk handed to a split part never ended for the lender: `PinnedBack` for any
   grant ended in a pinned area (sections 1 and 4).
6. `Elsewhere` before the grant made the edge ask back and forth, a guest's
   subscription takes `NotMine`, and the wrong region was named: subscriptions held as
   waiting while lending, `Elsewhere` and `NotMine` on `Lent` (section 5).
7. Effects already sent to the lender were not resent, and a return ended use at once:
   resent behind a mark with their due ticks counted again, and the return skips
   `end_uses` (section 3, steps 3 and 4).
8. One way for a whole line was too coarse, and a door fixed it: one way per stretch of
   eight chunk rows, and only a circuit's parts ask (sections 1 and 2).
9. A lend could move the line into the lender's circuit: the lender answers not
   lendable there (section 3, step 2).
10. Sections 1 and 7 disagreed on a part's line: neither (section 1).
11. Section 7 could not be computed from what the coordinator has: reports name the
    other region, the window rolls, and the split rule is put in `named`'s terms.
12. `lends_asked` through merges, splits and restores, and repeated asks: said
    (section 2).
13. The check at a restore needed chunks that were not loaded: lent chunks stay loaded,
    and what changes trigger the check is said (section 4).
14. What clears not lendable was undefined: said (section 2).
15. A state the table says is a signal source was ambiguous: the block data's
    `StateProps::is_signal_source` (section 2).

**Third review (2026-10-17): not ready.** It found the second's answers held against the
code save three, and these, worked in:

1. Dust dragged across the line made both sides refuse for good, each holding a part of
   the circuit in the chunk the other asked for: parts inside the chunks asked for do not
   count, and lends under way count for a stretch's way, so that no swap follows
   (sections 1 and 3).
2. Section 7's split rule could not fire, `named`'s squares lying far from the line: the
   line is `Sides::goes`'s (section 7).
3. A lender could not see a circuit in a chunk it had not loaded: it waits and loads
   (section 3, step 2).
4. A call-off crossing the lender's `Return` ended its use and dropped effects: the lender
   gives the chunk back by `Lending` with the lend's number, passed over when the lend is
   no longer under way (section 3).
5. The lender kept the effects it had received and would run them again later: dropped at
   `Lent` (section 3, step 4).
6. A `Loaded` answer could come after `Lent`: never taken for a lending chunk, and a load
   in flight is waited for (section 3, step 2).
7. "A split's part" switched section 7 off in an open world: "holds chunks of a pinned
   area by grant" (section 7).
8. `PinnedBack` goes once the release is durable (section 4).
9. The edge could go back and forth briefly: the borrower holds subscriptions while its
   ask is under way (section 5).
10. When the under-lend mark ends was unsaid: at the grant, a call-off or the lender's
    answer (section 2).
11. Step 4 did not tell the chunk's copiers when its use moved, and `Holdings` at a merge
    and a split is the simulation's own: both said (sections 1 and 3).

**Fourth review (2026-10-17): ready after small fixes.** Ten of the third's answers held;
these, worked in:

1. A wait answered for a lend called off could end a later lend's mark, leaving the
   lender's chunk lending for good: every message of a lend carries its number, and a
   `Lending` passed over is always answered `LendCalledOff` (section 3).
2. A lend under way the other way answered not lendable, which stuck where that lend was
   refused: answered wait (section 2).
3. The borrower held subscriptions for as long as an ask lived: only until the store's
   answer (section 5).
4. The wait-and-load could loop, and "ticking" might never hold on the line: loaded with
   stored ticks under a lend ticket kept until the next ask, and reach is the chunks
   beside (section 3, step 2).
5. The lender's trusted belief went stale when the borrower was reshaped: the store tells
   it the new holder (section 4).
6. The borrower stayed a copier of the chunk it held: dropped, and the lender added
   (section 3, step 4).
