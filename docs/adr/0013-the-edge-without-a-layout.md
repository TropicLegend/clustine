# ADR-0013: The edge without a layout

- Status: **Proposed**; the design of the edge's part of step C2b of milestone M3, phase
  C (step C2b.4 of ADR-0012). Not reviewed and not built yet.
- Date: 2026-10-08

## Context

Until now the edge works out from the layout which region a chunk, a player's position
or a block belongs to (`Layout::region_of`, in four places of `services/edge/src/
fanout.rs`): where it subscribes to a chunk, which chunks a hello names, where a
departed player goes and where a remote action goes.

[ADR-0012](0012-the-tick-on-chunks.md) takes the layout from the sim and the runner: a
region is the chunks the store grants it, and what a region says on a link tells the
edge who holds what. Its section 5 is the contract this record builds the edge against;
"rule n" below is a rule of that section. This record says what the edge keeps and what
it does in each case. It changes nothing a region does.

What stays as it is: connections, login, the encoding of packets, the routing of
inputs to the player's region, numbered messages and what is kept for a region
(ADR-0008), the resume with its presence answers, entities with the region that
introduced them (`Shown`), the patience with a region, and that `Fanout` is one task
that takes everything in turn.

## Decision

### 1. What the edge keeps

**Per region and chunk, a subscription**, in the region's `RegionPort`:

```rust
struct Subscription {
    /// What the region takes it for.
    kind: Kind,            // Viewer or Guest
    /// The number of the edge's last message on the current link that named the chunk;
    /// 0 for one the hello named.
    ask: u64,
    condition: Condition,  // Waiting, Served, or Elsewhere(RegionId)
    /// How many viewers whose player the edge believes to be this region's see the
    /// chunk. A subscription is a viewer's exactly while this is above 0.
    viewers: u32,
}
```

and per region the number of the last subscription message sent on its current link
(`asked`), which begins at 0 with every link.

**Per chunk of the replica**, beside the chunk and the count of all viewers that see it
(`ReplicaChunk::viewers`, as today): `served_by: Option<RegionId>`, the region whose
snapshot the replica has. It is set by a snapshot that is taken and cleared when that
region says `Elsewhere` or `NotMine` for the chunk or the edge ends its subscription
there; it is **not** cleared when a link ends (rule 16).

**The home region** comes from the routing table (`RoutingTable::home`), which the
coordinator fills from now on; a player who joins is the home region's. The layout is
no longer handed to the edge, and `Routing::new` takes the home region in its place.
Regions are still found by their index (`regions[region.0]`), which holds while regions
are the stripes; step C5 makes that a map.

Two statements hold after every turn of the task, and the code asserts them in tests:

- **V**: for every region `R` and chunk `c`, `viewers` of the subscription is the
  number of players of this edge whose `view.region` is `R` and whose `wanted` has `c`;
  and there is a subscription with `kind == Viewer` exactly where that number is above 0.
- **G**: a subscription with `kind == Guest` at a region `H` for `c` exists only if
  some viewer sees `c` (`ReplicaChunk::viewers > 0`), and either it is `Served`, or it
  is `Waiting` and some viewer's subscription to `c` at another region is
  `Elsewhere(H)`.

### 2. A viewer's view changes

`want(region, chunk)` and `unwant(region, chunk)` are the only two ways `viewers`
changes. They are called for every chunk that enters or leaves a player's `wanted`
with the player's region, and for every chunk of a player's view with the old and the
new region when the player's region changes (section 4), in that turn.

**`want(R, c)`** raises `viewers`. If it was 0:

- no subscription at `R` for `c`: one is made, `Viewer`, `Waiting`, and `Subscribe`
  is sent with the next number (rule 5);
- a guest's subscription there (`Served` or `Waiting`): it becomes a viewer's and
  `Subscribe` is sent with the next number (rule 6). A served one stays served and
  gets no answer (rule 9); a waiting one is answered under the new number.

**`unwant(R, c)`** lowers `viewers`. If it reaches 0 (rule 5):

- `Served`, and some viewer still sees `c`: the subscription becomes a guest's and
  `SubscribeAsGuest` is sent with the next number;
- otherwise the subscription is ended with `Unsubscribe` and forgotten; if it was the
  one that served the replica, `served_by` is cleared.

After either, **guests are tidied for `c`**: every guest's subscription to `c` that
statement G no longer allows is ended with `Unsubscribe`: a waiting one that no
viewer's subscription names any more, and every one when nobody sees `c`. When nobody
sees `c` the replica forgets the chunk, as today.

The subscription messages of one turn to one region are sent as one `Subscribe`, one
`SubscribeAsGuest` and one `Unsubscribe` at most, each with its own number, in the
order: `Unsubscribe`, `SubscribeAsGuest`, `Subscribe`. A chunk is named in one of them
only: what is left of several changes to one subscription within a turn is sent, and
`ask` is the number of the message that names it.

### 3. Answers

An answer whose `ask` is below the subscription's, or for which the edge has no
subscription at that region, is passed over (rule 8). Otherwise:

- **`ChunkSnapshot`**: the subscription is `Served`. The replica takes the chunk and
  its entities as today (reconciling one it has), `served_by` becomes this region, and
  the chunk goes to the viewers that want it.
- **`Elsewhere { region: H }`** (to a viewer's subscription): the condition becomes
  `Elsewhere(H)`; if `served_by` was this region it is cleared. If the edge has no
  subscription at `H` for the chunk, it makes a guest's one, `Waiting`, and sends
  `SubscribeAsGuest` (rule 13). If `H` is a region the edge has no port for, the
  answer is logged and nothing more is done; the routing table brings the region.
- **`NotMine`** (to a guest's subscription that was never served): the subscription is
  forgotten. Every viewer's subscription to the chunk that is `Elsewhere(this region)`
  is asked again: `Subscribe` with the next number, condition `Waiting` (rules 14 and
  15). So that two regions that each name the other cannot keep the edge busy, a
  subscription is asked again at most once per 5 ticks of the edge; one that is due
  later is asked by the tick that makes it due.

Until its snapshot comes, a chunk that a viewer wants and no region serves is not
shown, as a chunk that is not loaded yet is not shown today.

### 4. A player changes region

Where today `hand_over` finds the new region from the layout, it takes it from the
entry: `Departed { to }`, or the `holder` of a `NotMine` for an arrival (rules 18 and
20). In this order, all in one turn:

1. the view's subscriptions move: `unwant(old, c)` and `want(to, c)` for every chunk of
   the player's `wanted`, and the messages that follow from it are sent, to `to` first;
2. `view.region` becomes `to`;
3. `PlayerArrive` and the inputs above `transfer.last_input` are sent to `to` as
   numbered messages, as today, or `Discard` if the edge no longer has the player with
   that entity.

The subscriptions go first so that one claim of `to` covers the arrival's chunk and
the view (ADR-0012, section 5.1). A waiting subscription at the old region is ended by
step 1 (rule 5), so the `Elsewhere` that follows the `Departed` in that tick is passed
over by its number (rule 22).

A player who joins is the home region's from the join on; one who leaves is `unwant`ed
everywhere.

### 5. Blocks

- `Remote { action, to: Some(r) }` and `NotMine { what: Remote(action), holder: r }`
  are passed to `r` as `EdgeToWorker::Remote`, numbered, as a remote action is today.
- `Remote { action, to: None }` goes to `served_by` of the chunk of
  `action.step.concerns()`. If there is none, or it is the region the entry came from,
  or the edge no longer has the player, the action ends at the edge as today
  (`Fanout::arrived`), or is dropped (rule 26).

### 6. A link ends, and a new one begins

When a link ends, every subscription of the region keeps its kind and its viewers;
its `ask` and the region's `asked` become 0 and its condition `Waiting`. `served_by`
stays. The hello on the next link names the viewer's subscriptions as `chunks` and the
guest's as `guests` (rule 2); answers come under the number 0.

When a region answers `Unknown` and the edge gives up what it kept for it (ADR-0008,
section 5), the players of that region are removed, which `unwant`s their chunks; the
subscriptions that are left are guests', and stay.

### 7. Building it

The edge of C2b.4 has to work against regions that presume (ADR-0012, section 8) and
regions that ask the store; both are run end to end from C2b.3 on.

| # | Scope | Its tests |
|---|---|---|
| E1 | `ask` on subscription messages and answers, counted per link, stale answers passed over; the subscription table with kinds, conditions and viewers, still filled by the layout | The edge's unit tests on scripted regions; statements V and G checked after every step of every test |
| E2 | `want` and `unwant`, guests, `Elsewhere`, `NotMine`, `served_by`; the hello's two lists; a link that ends | Below, 1 to 9 |
| E3 | `Departed { to }`, `NotMine` for an arrival, `Remote` by `to` and by `served_by`; the home region from the routing table; the layout goes from the edge | Below, 10 to 15; the end-to-end tests, presumed and asking |

Scenarios for whoever writes tests from this record alone, on scripted regions as
`fanout.rs` has them (`Harness`):

1. A player joins: the home region is sent one `Subscribe` naming every chunk of the
   view, and no other region is sent anything.
2. `Elsewhere { region: H }` for a chunk makes one `SubscribeAsGuest` to `H`; a second
   `Elsewhere` naming `H` for the same chunk, from another region's viewer's
   subscription, makes none.
3. A snapshot from `H` is shown; one from a region the edge has no subscription at,
   or with a number below the subscription's, is not.
4. A player walks so that a chunk leaves the view: `Unsubscribe` to their region, and
   to the region that served the chunk as a guest if nobody else sees it.
5. Two players of two regions see one chunk that the first one's region serves: the
   second region is told nothing of it but its own `Subscribe`, which is answered
   `Elsewhere`; when the first player leaves, their region is sent
   `SubscribeAsGuest`, not `Unsubscribe`, and the chunk stays shown without a new
   snapshot.
6. `NotMine` from a guest's region makes one `Subscribe` again at the viewer's region,
   under a new number; a second `NotMine` within 5 ticks makes the next one only when
   those are over.
7. A stale answer after a change of kind (both crossings of ADR-0012's review, defect
   1) changes nothing.
8. A link ends and a new one comes: the hello's `chunks` and `guests` are the
   subscriptions by kind; answers numbered 0 are taken; the chunk stays shown
   meanwhile and a `Remote` without a region still goes to the region that served it.
9. A region answers `Unknown`: its players are gone, its viewer's subscriptions with
   them, and guests' that other regions' viewers need are in the next hello.
10. `Departed { to }`: `to` is sent the `Subscribe` for the view before `PlayerArrive`,
    the old region `Unsubscribe` or `SubscribeAsGuest` per chunk, and an `Elsewhere`
    from the old region for the chunk the player stepped into, read afterwards, changes
    nothing.
11. `NotMine` for an arrival is handled as a `Departed` to its holder, also back to the
    region the player came from.
12. `Remote` with a region goes there; without one, to the region that serves the
    chunk; without one and served by the region it came from, or by nobody, it is
    acknowledged to the player.
13. A player who left before their `Departed` is read: `Discard` goes to `to`.
14. Statements V and G hold after every step of a random sequence of joins, moves,
    hand-overs, answers and lost links, for three regions and four players.
15. The end-to-end tests of hand-over, blocks, takeover, chaos and moves, with regions
    that presume and with regions that ask.

## Consequences

- The edge no longer knows how the world is divided; step C5 can take the layout away
  from the rest.
- A chunk another region holds takes a round more to appear the first time: the
  viewer's region answers `Elsewhere`, and the holder then sends the snapshot.
- The edge holds more per chunk: a subscription for each region that has a viewer of
  it, and one for a guest.

## Open questions

1. Whether five ticks between two askings of one subscription is right; in a world of
   pinned regions it never comes to it.
2. What an edge does with a region an `Elsewhere` names that the routing table does
   not have yet (step C3 makes such regions).
