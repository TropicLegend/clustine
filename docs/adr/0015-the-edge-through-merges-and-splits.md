# ADR-0015: The edge through merges and splits

- Status: **Accepted**; the design of the edge's part of step C3 of milestone M3, phase
  C (step C3.7 of ADR-0014), against section 8 of
  [ADR-0014](0014-merging-and-splitting.md), rules 34 to 50. Written by whoever builds
  it, and revised after an independent review against the code and that contract (see
  "Review"). Built, and tested from this record by someone who did not read the code;
  what that found is at the end.
- Date: 2026-10-08

## Context

Regions merge and split from step C3 on (ADR-0014). The edge
([ADR-0013](0013-the-edge-without-a-layout.md)) knows regions only as ports: for each
region it has had to do with, a link or none, what it kept for the region, how far the
two have got with each other's numbers, and its subscriptions there. A merge takes a
region away for good and puts its players, chunks and outbox into another; a split
makes a region nobody has heard of and puts players into it without the edge having
sent them. ADR-0014 decided how the edge hears of both, and this record builds on
these decisions of it:

- **One setting.** A region that absorbs or is split closes every link. The edge
  meets `Absorbed` and `SplitOff` among the entries of a welcome and nowhere else:
  after its hello, with every subscription of that link begun anew, before the
  presence answers, and before it has sent the region anything it kept.
- **Stays.** A player's time in the world from a join to the leave that ends it has
  one entity id, and what the edge says about a player names it: `Input { player,
  entity, .. }`, `PlayerLeave { player, entity }`. A region passes over what is for a
  stay it does not have.
- **The region says whom it has.** After a welcome's entries come `presences`
  presence answers: one for each player the hello named, then `Present` for every
  other stay the region has for the edge. The welcome also says `applied`: how far
  the region had applied the edge's messages in the state those answers are made of.
- **The hold** (rule 34): a block action sent behind a subscription for its chunk, on
  the same link, is judged only when the region has the chunk loaded, if the region
  holds it or comes to hold it.

What the edge is today, where this record changes it (`services/edge/src/fanout.rs`):

- `welcomed` reads the welcome, forgets the region after an `Unknown` if the two had
  shared anything, and sends what was kept once the announced entries are handled.
- `handle_entry` passes over an entry numbered at or below `RegionPort::seen`,
  handles the others, and confirms. `Absorbed` and `SplitOff` are logged and
  confirmed.
- `presence` handles an answer only for a player the edge has under the region that
  answers; a `Present` with another entity than the edge's disconnects the player.
- `hand_over` takes a `Departed` that names the region it came from for a fault and
  disconnects the player; `pass_on` ends an action that would go back where it came
  from.
- `PlayerView` is one stay: it is made by a join and dropped by `remove_player`, and
  has `entity: Option<EntityId>`, `None` until a region has placed the player.
- The routing table reaches the process, not the task: `whole_world` reads the first
  one and `keep_linked` the later ones (`bin/clustine/src/cluster.rs`), and the task
  is handed links through `Relinks`.

## Decision

Names: `A` survives a merge or is split, `B` is absorbed, `N` is the part. "The edge
has the stay `(P, e)` under `R`" means `players[P].entity == Some(e)` and
`players[P].region == R`.

### 1. What the edge keeps, further

```rust
struct Fanout {
    // The regions that are no more, each with the region it went into, as far as the
    // edge has acted on it. Never a chain: a value is not itself a key. Written by
    // `retire` and by nothing else.
    stands_for: BTreeMap<RegionId, RegionId>,
    // ...
}

struct RegionPort {
    // The entries of this region's outbox that came to it with a merge: for each, by
    // its number here, the region whose entry it was and the number it had there.
    // An entry is taken out when it has been handled or passed over. Emptied
    // wherever `seen` is put to 0.
    came_with: BTreeMap<u64, (RegionId, u64)>,
    // The regions the routing table says went into this one, of which the edge still
    // has something and for which it has handled no `Absorbed` (section 5).
    owes: BTreeSet<RegionId>,
    // ...
}

struct Link {
    // How many presence answers the welcome announced are still to come; `None`
    // until the welcome has been read.
    presences: Option<u32>,
    // The players this welcome itself made the region's (by an `Absorbed` among its
    // entries, or by the conclusion of section 5) and for whom no `Present` has come.
    // They are judged when the presence is through (section 2.2).
    brought: BTreeSet<PlayerId>,
    // The port's `owes` as it was when the hello of this link was said: only for
    // these does the end of its entries say that an `Absorbed` is not coming.
    owed: BTreeSet<RegionId>,
    // ...
}
```

`fn living(&self, region: RegionId) -> RegionId` is `stands_for[region]` if there is
one and `region` otherwise. **Every region id that a region says** (`Departed::to`,
`Remote::to`, `NotMine::holder`, `Elsewhere::region`, `SplitOff::region`, and the
target of a pair of the routing table) **goes through `living` where it is read**.
The one exception is `Absorbed::region`, which is the key being made.

**`retire(B, A)`** is the one place a region becomes a key: `stands_for[B] = A`;
every value `B` in `stands_for` becomes `A`; `A`'s `owes` loses `B`; and **`B`'s link
is taken**, if the port still has one (`port.link.take()`, and the word on `lost`), so
that whatever that link still delivers is dropped by the run loop like the messages
of any link that is no longer the region's. A link's end is read from the same queue
as everything else, with no order against another region's link, so the port of an
absorbed region can well have its link, with messages unread, when the edge learns of
the merge. What was unread on it and matters comes again behind the `Absorbed`.
`take_link` drops a link for a region that is a key.

A port of a region that is a key of `stands_for` is a **tombstone**: no link, no
players, no subscriptions, nothing kept. It keeps `seen` only because an entry that
came to the survivor with a merge is told from one the edge has handled by it.

**Statement S**, checked with V, G and E at the end of a turn: for every key `B` of
`stands_for`: no player is under `B`; port `B` has no link, no subscription and
nothing kept; no subscription anywhere is `Elsewhere(B)`; no `Shown::from` and no
`served_by` is `B`; and `stands_for[B]` is not a key.

### 2. Presence

`welcomed` notes `presences` with `entries`, and takes the welcome's `applied` as
`progress` takes a region's word: what is kept for the region up to that number is
dropped. (`Welcome::Unknown` forgets the region first, as today, which removes the
players the edge had there and empties `came_with`; its `applied` is then 0 or the
number of a state that has received nothing.) An answer is counted when it is read,
whatever is done with it; when the count reaches 0 the link's presence is **through**
(section 2.2).

#### 2.1 An answer

`Presence::Present { entity: e, last_input, handled, .. }` for `P` from `R`:

1. **The edge has the stay `(P, e)` under `R`**: as today (the kept inputs up to
   `last_input` are dropped; `handled` goes through the acknowledging).
2. **The edge has `P` under `R` with no entity yet** (they are entering the world):
   - if a `PlayerJoin` of `P` is still among what is kept for `R`, the region had not
     applied it when it made the answer, and the answer is about a stay from before
     the join, which the join will end (ADR-0014, section 2.1): nothing is done;
   - otherwise the region applied the join, and the word of it was lost with a link:
     they enter the world as `e`, as today.

   This takes the place of today's test for a kept leave, which is wrong where a
   leave was applied or passed over by a region the stay was not in: a split had
   taken it elsewhere, and a merge brought it back.
3. **The edge has the stay `(P, e)` under another region `X`**: the stay is `R`'s.
   `view.region = R`; the kept inputs up to `last_input` are dropped; for every chunk
   of the view `unwant(X, c)`, then for every chunk `want(R, c)`, and the messages are
   sent (as `hand_over` does); then every input still kept is sent to `R` as `Input {
   P, e, number, input }`; `handled` as in case 1. There is no `PlayerArrive`. **What
   `R` says of the entity `e` counts from then on** (`Shown::from` becomes `R`, if
   the edge shows the entity): `R` does not introduce an entity it has had all along
   before it says where it moves, and a tick's events come before its snapshots.
4. **Anything else** (the edge has no `P`; or has `P` with another entity; or has `P`
   without an entity under another region): `PlayerLeave { player: P, entity: Some(e)
   }` to `R`. The edge's own stay of `P`, if it has one, is untouched. (Today a
   `Present` with another entity disconnects the player the edge has; it no longer
   does: the region's stay is the one that ends.)

In cases 1 to 3 `P` is taken out of the link's `brought`.

`Presence::Absent` for `P` from `R`: if the edge has `P` under `R`, as today: the
player is disconnected unless a `PlayerJoin` or a `PlayerArrive` of theirs is among
what is kept for `R`.

#### 2.2 When the presence of a link is through

Every player of the link's `brought` whom the edge still has under `R` is judged **as
after `Absent`**. Nobody else is: a player the hello named has had an answer of their
own (rule 37), and a player whom something else put under `R` meanwhile (a `Departed`
or a `SplitOff` of another region, read between this link's messages) is not this
welcome's to judge. Such an entry can be older than what `R` has done with the stay
since; the presence of whoever has the stay moves it (case 3), or the edge's patience
ends it.

Why case 3 may trust the region is ADR-0014, rule 38: a stay is in one region's state
at a time, and a region has a stay the edge did not send it only by a merge or a
split.

### 3. `Absorbed`

`Durable::Absorbed { region: B, since, applied, numbers }`, the entry numbered `n` of
`A`'s outbox, is handled like this and in this order.

1. **Shared numbering.** With `b` the port of `B` (made if there is none):
   - `shared` if `since != 0 && since == b.since`;
   - else, if `b.seen != 0 || b.applied != 0`, `B` **had forgotten the edge**: what
     is kept for `B` is given up as `forget_region` gives it up (a `Remote` among it
     is told to its player as handled), `b.numbered`, `b.applied` and `b.seen` are 0,
     `b.came_with` is emptied, and `applied` counts as 0 below. The players under `B`
     are not disconnected here: step 3 makes them `A`'s and section 2.2 judges them;
   - else nothing the edge kept for `B` was applied: `applied` counts as 0.
2. **`retire(B, A)`.** (`B` can be a key already, by the first case of section 5: the
   edge then had nothing of `B`, and the steps below find nothing to move.)
3. **Players and their views.** Each player under `B`, in ascending order, is put
   under `A` and into the link's `brought`, and their view moves as at a hand-over:
   `unwant(B, c)` for every chunk of the view, then `want(A, c)` for every chunk
   (ADR-0013, section 2). Nothing is said to `B`, which has no link; on `A`'s link
   that is a `Subscribe` for each chunk `A` was not asked for as a viewer already.
4. **What is left at `B` are guests' subscriptions**, for chunks that players of
   other regions see. For each, in ascending order of the chunk: if `A` has no
   subscription for the chunk, one is made (`Guest`, `Waiting`) and `SubscribeAsGuest`
   is to be sent. `B`'s subscriptions are then dropped without a message.
5. **Whatever named `B`.** Every subscription that is `Elsewhere(B)` becomes
   `Elsewhere(A)`: statement E holds for it, as after steps 3 and 4 `A` has a
   subscription wherever `B` had one, and `A` serves the chunk or says `NotMine`,
   which has the region asked again as ever. Every `Shown::from` that is `B` becomes
   `A`, and every `served_by` that is `B` becomes `A`: `A`'s snapshots put right
   what `B` showed, and an action without a region named goes to `A`, behind the
   subscription. **The subscription messages of steps 3 and 4 are sent now**
   (`flush_asking`), so that they are on the link before anything of step 6.
6. **What was kept for `B`** with a number above `applied` is kept for `A`: each, in
   order, gets `A`'s next number and goes to the end of `A`'s `kept`. The rest of
   `B`'s `kept` is dropped (`B` had applied it; it is in the state `A` took over).
   `A`'s link is not `welcomed` while its entries are being read, and `send_kept`
   sends everything in order when they are through, behind the subscriptions of
   steps 3 and 4 (rule 34). Should the link be `welcomed` all the same, what is
   moved is sent at once.
7. **The entries behind.** For `i` in `0..numbers.len()`: `A.came_with[n + 1 + i] =
   (B, numbers[i])`; but an entry that `B` itself had come by with a merge (it is in
   `b.came_with` under `numbers[i]`) keeps the origin it had there. `b.came_with` is
   then emptied.
8. The entry is confirmed, like any other.

**An entry of `A` that is in `came_with`**, as `(O, m)`: it is **passed over** (seen,
confirmed, not acted on) if the edge had handled it as `O`'s: `m <= port(O).seen` (a
numbering that was given up has `seen` 0, so nothing of it is passed over). Otherwise
it is handled as an entry of `A` **that may send a thing back to `A`** (section 4).
Either way it is taken out of `came_with`. An `Absorbed { region: C, .. }` among them
is handled by this section with `A` as the survivor; its `numbers` are `C`'s.

`came_with` and `stands_for` outlive links: a link that ends between an `Absorbed`
and the entries behind it brings the rest in the next welcome, without the `Absorbed`
in front (`A.seen` is past it), and they are still told by their numbers.

### 4. Entries that name the region they come from, and actions sent on

A region id an entry names is read through `living`. That can make it the region the
entry came from. Until now that is a fault of the region (it named itself) and the
player is disconnected. It is right in two cases:

- **the entry is in `came_with`**: it was `B`'s and names `A`, or names a region that
  stands for `A` (`B` let the player go to `A` before the merge, and `A` now has what
  `B` was to tell the edge);
- **the entry names another region than the one it comes from**, and that region
  stands for it: `A`'s own `Departed { to: B }` from before the merge, read when the
  edge has `B` standing for `A` already (by the first case of section 5).

So: **`hand_over` with `living(to) == from` is an arrival at `from` in those two
cases, and the fault of today otherwise.** An arrival at `from`: the check of the
stay as today (the edge has it under `from`); the kept inputs up to
`transfer.last_input` are dropped; `PlayerArrive` and the inputs still kept go to
`from`; the subscriptions stay where they are. `pass_on` sends a `Remote` back to the
region it came from in the same two cases, and ends it otherwise.

An entry of `A` numbered before its `Absorbed { B }` that names `B`, read while `B`
is not a key, needs none of this: the player is put under `B`, the arrival is kept
for `B` and the view is asked of `B`, all without a link, as for any region the edge
cannot reach; the `Absorbed` that follows brings the three to `A` by section 3. This
is rule 39's "kept for `B`, as for any region without a link".

`hand_over`'s first check stays: a `Departed` for a stay the edge does not have with
that entity is answered with `Discard` to `living(to)`.

**An action is never sent to a region the edge is not asking for its chunk.**
`pass_on`, before it sends a `Remote` to `R`: if the edge has no subscription at `R`
for the chunk the action's step concerns, and someone sees that chunk, it makes a
guest's (`SubscribeAsGuest`, sent first, on that link). Rule 34 then holds the action
at a region that holds the chunk and has yet to load it, and a region that does not
hold it says `NotMine` to the guest and passes the action on as ever. Without this, an
action a region sends on to a part right after a split can reach the part before the
edge has asked it for the chunk, and be judged on a chunk that is not loaded.

### 5. The routing table's pairs, and an `Absorbed` that does not come

`Relinks` gains `absorbed(pairs: Vec<(RegionId, RegionId)>)`. The edge's process calls
it with the `absorbed` of the first table, before it hands over any link, and of every
table that arrives after it, **whatever else it does with that table** (one whose
layout it refuses as well). The task takes it in its loop like a new link.

For each pair `B → A'`, with `A = living(A')` after following the pairs to a region
that is not itself absorbed by them, and `B` not a key of `stands_for`:

- **if the edge has nothing of `B`** (no player under it, no subscription at it,
  nothing kept for it, no subscription `Elsewhere(B)`, no `Shown::from` or
  `served_by` that is `B`): `retire(B, A)` at once. There is nothing an `Absorbed`
  could move. A link `B`'s port still has does not count as something: `retire` takes
  it.
- **otherwise** `B` is put into `A`'s `owes`. If `A` has a link whose entries are
  through and whose `owed` lacks `B`, the edge ends that link (`lose_link`), so that
  a hello is said that `A` answers from after the merge. (The link may be one from
  before the merge whose end the edge has not read yet, or one from after it whose
  welcome had no `Absorbed { B }`; the edge cannot tell which, and a hello settles
  it.) This cannot go round: the next link's `owed` has `B`.

`take_link` copies the port's `owes` into the link's `owed`.

**When the entries of a welcome are through** (in `outbox` when the last announced
entry has been handled, and in `welcomed` when none was announced), in this order:

1. For every `B` in the link's `owed` that is still in the port's `owes` (no
   `Absorbed { B }` was handled, on this link or before): `B` had forgotten the edge,
   or `A` has since (rule 44). What is kept for `B` is given up as in the second case
   of step 1 of section 3, and steps 2 to 5 of section 3 are done, with their
   subscription messages.
2. If the port's `owes` still has a region (a pair came while this welcome was being
   read), the link is ended as above, and nothing below is done.
3. What was kept is sent (`send_kept`).
4. If no presence answers are to come (`presences` is 0), the presence is through
   (section 2.2).

The pairs are used for nothing else. In particular a name of `B` in an entry is `B`
until `retire` has made it a key.

### 6. `SplitOff`

`Durable::SplitOff { region: N', players }` from `A`, with `N = living(N')`. For each
`(P, e)` of `players`, in order: **if the edge has the stay `(P, e)` under `A`, and no
`PlayerArrive` of `P` is among what is kept for `A`**: `view.region = N`; for every
chunk of the view `unwant(A, c)`, then `want(N, c)`, and the messages are sent; every
input kept is sent to `N` as `Input { P, e, number, input }`; and what `N` says of the
entity `e` counts from then on, as in case 3 of section 2.1. Anything else: nothing.
The entry is confirmed.

The entry says where the stay was when `A` was split, and is read at any time after.
A stay the edge has under `A` with an arrival kept for `A` has come back since: the
part's presence moved it to the part (case 3), it walked back, and its arrival waits
for this very link. Nothing else can have put it under `A` again before the entry is
read, as the entry is among a welcome's entries and nothing kept is sent, so nothing
reported applied, before those are through.

`N` gets its port by this (ADR-0013, section 1), and its link when the routing table
has its route. The hello names the players under `N` and their views, so the hold of
a hello covers what they do. `N`'s welcome is `Unknown { since, entries, presences,
applied: 0 }`, which `welcomed` takes as a beginning, since the edge never had
anything from `N`; `entries` is not 0 if `N` has been split or has absorbed since.
The presence answers are case 1 for each stay, or case 3 if the link to `N` came
before `A`'s `SplitOff` was read, after which the `SplitOff` finds the stays under `N`
and does nothing.

`A`'s own presence answers `Absent` for a player its hello named who went to `N`. By
then the edge has them under `N` and passes the answer over.

### 7. Inputs and leaves name the stay

- `Command::Input`: an input of a player without an entity is dropped (the client is
  not in the world before a region has placed it). Every `Input` the edge makes or
  sends again carries `view.entity`.
- `remove_player` sends `PlayerLeave { player, entity: view.entity }` to
  `view.region`.
- A leave made by case 4 of section 2.1 names the region's entity.

### 8. What this rests on, and does not make sure of itself

- **`hand_over` passes over a `Departed` for a stay the edge has under another
  region**, as today. With stays that move without the edge such an entry could be
  true, and the stay would then belong to nobody. It cannot be reached while three
  things hold, which no rule of the contract promises and which steps C4 and C5 must
  not undo without coming back here: a part holds the chunk each of its players
  stands in; a stay does not leave a region without an input of this edge; and a
  merge announces itself in the survivor's outbox before anything the survivor says
  of a stay that came with it.
- **A hello said after a pair is in the routing table is answered from after the
  merge** (section 5 concludes from it). It holds because the coordinator reads of a
  merge only once the record is on disk, and a runner answers no hello between
  handing the store a merge and taking it: taking it drops every link, also those
  attached and not yet taken up (ADR-0014, section 3.3).
- **A `Present` read from a link from before a split**, after the part's presence has
  moved the stay to the part, moves it back (case 3). The `SplitOff` on the next link
  to the split region puts it right.
- **With more than one edge**, an entity in a chunk of a part keeps `Shown::from` as
  the split region, and the part's snapshots cannot remove it. Section 3 rewrites it
  for a merge; nothing does for a split. It goes with the open point of ADR-0008 and
  ADR-0013 about entities and several edges.

### 9. Building it

| # | Scope | Needs | Tests |
|---|---|---|---|
| F1 | Section 7; `presences` and `applied` read and not used. Mechanical, in the commit of step C3.1 | C3.1 | the edge's own, unchanged in what they assert |
| F2 | Section 2: `applied` taken, presence by count, the four cases, the judgement of those a welcome brought | C3.1 (it works with a runner that announces only the hello's names, and with one that says every stay) | 1 to 6 below |
| F3 | Sections 1, 3, 4, 5, 6: `stands_for`, `retire`, tombstones, statement S, `Absorbed`, entries that came with a merge, the guest's subscription before a `Remote`, the pairs through `Relinks`, `SplitOff` | F2; `Relinks::absorbed` is called by the edge's process (`whole_world` and `keep_linked` in `bin/clustine/src/cluster.rs`, which step C3.6 touches only for `is_complete`) in the same commit | 7 to 30 below; A1 to A9 of ADR-0014 |

Scenarios, against scripted regions, for someone who has this record, ADR-0013 and
ADR-0014's section 8 and not the edge's code. "Resumes with `R`" is: a new link to
`R`, the hello read, a welcome scripted. Where two links are involved, **each
scenario is run with their messages interleaved in every order that keeps each
link's own**, not only with one whole welcome after the other.

1. A welcome announces two answers and the hello named one player: `Present` for the
   one named, `Present` for a stay `(Q, e)` the edge does not have: `PlayerLeave { Q,
   Some(e) }` goes to that region and nothing else changes.
2. `Present { P, e2 }` where the edge has `(P, e1)` under that region and the hello
   named `P`: `PlayerLeave { P, Some(e2) }`; `P` is not disconnected by it. With
   `Absent` for `P` instead, they are, unless a join or an arrival of theirs is kept.
3. `Present { P, e }` from `R` where the edge has `(P, e)` under `X`: no
   `PlayerArrive`; the view is asked of `R` as a viewer's before any `Input`; the
   inputs above `last_input` go to `R` with `e`; at `X` the view's subscriptions
   become guests' or end as ADR-0013 section 2 has it; an input the player makes next
   goes to `R`.
4. A player enters the world; the welcome says `applied` at or above the number of
   their join, and `Present { P, e }`: they are placed as `e`. With `applied` below
   the join's number and `Present { P, e1 }`: nothing is done, the join is sent again,
   and the `Spawned { P, e2 }` that follows places them as `e2`; their inputs name
   `e2`.
5. The sequence of the review's defect 2: `P` leaves, the leave is applied by a
   region that no longer has the stay, a merge brings the stay back, `P` joins again:
   `P` ends as the new stay, and the old one is gone from the region (by the join).
6. A welcome's `applied` drops what was kept up to it before anything else is done:
   a `PlayerJoin` numbered at or below it is not sent again.
7. `Absorbed { B, since: s, applied: 3, numbers: [] }` among `A`'s entries, the edge
   holding `since` `s` for `B`, messages 2 to 5 kept for `B`, a player under `B` with
   a view, a guest's subscription at `B`: on `A`'s link: `Subscribe` and
   `SubscribeAsGuest` for what was at `B`, in either order, both before anything
   numbered; and when the entries are through the kept messages of `A` followed by
   `B`'s 4 and 5 under `A`'s next numbers. The player's next input goes to `A`.
   Statement S holds.
8. The same, and the welcome's presence has no `Present` for that player: they are
   disconnected when it is through, with a leave that names their entity. With a
   `Present` for them: they stay.
9. The same with `since` another than the edge holds and something seen of `B`:
   nothing of `B`'s kept is sent; a `Remote` among it is acknowledged to its player.
10. The same where the edge never had anything from `B`: everything kept for `B`
    goes to `A`.
11. Entries behind an `Absorbed` with `numbers: [4, 5, 6]`, the edge having seen 5 of
    `B`: the first two are confirmed and not acted on, the third is handled.
12. One of them is `Departed { P, to: A }` (it was `B`'s): `P` arrives at `A` with a
    `PlayerArrive` and their kept inputs, and is not disconnected. An entry of `A`'s
    own that names `A` disconnects the player, as today.
13. `A`'s own `Departed { P, to: B }` numbered before the `Absorbed { B }`: `P` is
    under `B` after the first and under `A` after the second, and the `PlayerArrive`
    reaches `A` under `A`'s numbers.
14. A link that ends after the `Absorbed` and before the entries behind it: the next
    welcome brings them without it, and 11 holds as before.
15. `A` then forgets the edge (`Unknown`) with entries of that kind still to come:
    an ordinary entry of `A` under a number those had is handled as `A`'s own.
16. `B` absorbed `C`, then `A` absorbs `B`, no link meanwhile (A2 of ADR-0014): after
    `A`'s welcome everything the edge had at `B` and at `C` is at `A`; an entry of
    `C`'s the edge had seen at `C` is passed over.
17. An `Absorbed { B }` is read while the edge's link to `B` still stands with an
    entry unread on it, which is also among the entries behind the `Absorbed`: the
    entry is acted on once.
18. A viewer's subscription at a third region told `Elsewhere(B)`, asked again within
    the last second: after `Absorbed { B }` it is `Elsewhere(A)`, statements E and S
    hold, and `NotMine` from `A` has the third region asked again. A later `Elsewhere
    { region: B }` from any region is taken as naming `A`.
19. The pairs say `B → A` while the edge has a player under `B` and a link to `A`
    whose entries are through: that link is ended; the welcome of the next has no
    `Absorbed { B }`: what was kept for `B` is given up, the player is `A`'s and
    judged by that welcome's presence, also when it announces no answers.
20. The pairs arrive first, the welcome with `Absorbed { B }` after: as 7, and no
    link is ended.
21. The pairs arrive while a welcome of `A` is being read that has no `Absorbed { B
    }`: the link is ended when its entries are through and nothing kept was sent on
    it.
22. The pairs for a region the edge has nothing of but a link: the link is taken,
    nothing is sent, and a later `Departed { to: B }` from a third region is an
    arrival at `A`.
23. `B` absorbed `C` (handled), then the pairs say `B → A` with nothing of `B`: a
    later `Departed { to: C }` puts the player under `A`.
24. `A` absorbed `B`, then `D` absorbed `A`; the edge has a player under `B`, a link
    to `A` with its welcome unread, and the pairs come first: after `D`'s welcome the
    player is under `D`.
25. A table whose pairs name as target a region the edge has already retired (it is
    ahead of the table): the pair is taken as naming the living region.
26. `SplitOff { N, [(P, e), (Q, f)] }` where the edge has `(P, e)` under `A` and no
    `Q`: `P`'s view is asked of `N` in the hello of `N`'s first link, and `P`'s kept
    inputs are sent there after its welcome, numbered from 1; nothing is said about
    `Q`; `Present { Q, f }` in `N`'s presence is answered with `PlayerLeave { Q,
    Some(f) }`.
27. The link to `N` comes before `A`'s `SplitOff` is read: the hello names nobody,
    `Present { P, e }` moves `P` as in 3, and the `SplitOff` then changes nothing.
28. The same, and `P` walks back before the link to `A` (the sequence of the review's
    defect 1): `N` says `Departed { P, to: A }`, the arrival is kept for `A`, and
    `A`'s `SplitOff` leaves `P` under `A`.
29. Two splits unread (defect 3): `A` split `P` and `Q` into `N`, `N` split `P` into
    `N2`; with the links to `N` and `A` read interleaved, nobody is disconnected, and
    after `N2`'s presence `P` is under `N2`.
30. A3 of ADR-0014 interleaved (defect 6): the part was absorbed by `C` and the edge
    has handled that before it reads `A`'s `SplitOff { N }`: `P` is put under `C`.
31. A `Remote` that a region sends on to a region the edge has no subscription at:
    `SubscribeAsGuest` for its chunk goes out on that link before the `Remote`.
32. Statements V, G, E and S hold after every step of generated runs in which regions
    merge and split while links are lost (the generated run of ADR-0013's tests, with
    a model of ADR-0014's sections 2 and 3 for the regions).

## Consequences

- The edge acts on a merge only when the survivor tells it, and on the routing table
  only to know that it is owed that word, or that there is nothing to be told. A
  table that is ahead of the survivor costs one resume.
- A presence answer can move a player between regions. Until now only an outbox
  entry could.
- A region's stay that the edge does not know is ended by the edge at the next hello,
  whatever was lost on the way. The disconnect "a region has a player as another
  entity" goes.
- An action sent on to a region costs a guest's subscription there if the edge had
  none.
- Ports of absorbed regions stay as tombstones for the life of the edge: a few
  numbers each.

## Changes to ADR-0013

- Section 4: a `Departed` whose destination is the region it came from is an arrival
  there under the condition of section 4 here; an action is sent on only behind a
  subscription for its chunk.
- Section 6: `Link` and `RegionPort` gain the fields of section 1; a welcome's
  presence is counted, and its `applied` is taken.
- Statement S joins V, G and E.

## Changes to ADR-0014

- **Welcomes say `applied`** (sections 3.7 and 9; rules 37 and 38): `Welcome::Resumed
  { entries, presences, applied }`, `Welcome::Unknown { since, entries, presences,
  applied }`: the number of the edge's last message the region had applied in the
  state the entries and answers are made of. A `Present` for a player who is entering
  the world is the stay of their join only if the join is at or below it.
- **Rule 38**: when the answers are through, only the players that welcome itself made
  the region's are judged, not everyone the edge believes to be there.
- **Rule 39** names `SplitOff::region` among what is read through the stand-ins, and
  allows the edge to let a region of which it has nothing stand for its survivor on
  the routing table's word.
- **Rule 40** goes: a subscription told elsewhere with `B` is told elsewhere with `A`
  from the `Absorbed` on, and `A` is asked for the chunk as a guest already.
- **Rule 44** rests on a runner answering no hello between handing the store a merge
  or a split and taking it, which section 3.3 gives and the rule now says.
- **Rule 45**: a `SplitOff` does not move a stay of which an arrival is kept for the
  region that says it.
- **Rule 47**: the first welcome of a new region has entries if it has been split or
  has absorbed since.
- **Rule 48**, fourth item: the edge asks the region an action is sent on to for the
  chunk first (section 4 here), so the action is held there whatever became of the
  `Elsewhere` that named it.
- **Rule 50**: "the order in which it resumes with the regions does not matter" holds
  for whole welcomes and for their messages interleaved, by the three changes above.

## Open questions

1. Whether ending the link in section 5 is worth a rule of its own, or whether the
   pairs should simply wait for the next link. As written a table that is ahead of
   the survivor is settled at once; waiting would leave `B`'s players standing until
   the link to `A` happens to end.
2. The third case of step 1 of section 3 sends `A` everything kept for a `B` the edge
   "never had anything from", also what an earlier life of `B` applied before it
   forgot the edge, if no `Progress` of it was read. `welcomed` has the same test
   today for `Unknown`.

## Review

An independent review against the code and against section 8 of ADR-0014 found eleven
defects in the first version of this record, three of which disconnected a player who
had done nothing wrong, and found sound: the order on the survivor's link
(subscriptions before what was kept), statements V, G and E through the moving of
views in every condition of the absorbed region's port, `came_with` through nested
merges, that ending a link over the pairs cannot go round, and that the two cases of
section 4 are all there are. What it found, and what was decided:

1. A `SplitOff` took back a stay that had returned: the part's link came first, the
   player walked back, and the split region's welcome then moved them to the part
   again. A `SplitOff` does not move a stay whose arrival is kept for that region.
2. A `Present` for a player who is entering the world could be an older stay with no
   leave kept (a split had taken the stay from under the leave, a merge brought it
   back), and the player was placed as their old self. The welcome says how far the
   region had applied, and the answer counts only if the join is within that.
3. The judgement at the end of a presence disconnected a player whom another region's
   old `SplitOff` had put under the region meanwhile. Only those the welcome itself
   brought are judged.
4. Nothing took the link of a region that was no more, and "nothing of it" did not
   look at one: an entry could be acted on twice, and two merges in a row with the
   pairs first left players under a tombstone. `retire` is the one place a region
   becomes a key, and takes the link.
5. The pairs could make a chain of stand-ins. `retire` rewrites the values.
6. `SplitOff::region` was not read through `living`.
7. Statement S was false where a subscription told elsewhere with the absorbed region
   had been asked within the last second. It becomes told elsewhere with the
   survivor; asking again went.
8. `came_with` outlived the numbering it belonged to. It is emptied wherever `seen`
   is put to 0.
9. What happens when a welcome's entries are through had no order, and two sections
   disagreed about a welcome without answers. Section 5 has the order.
10. The first routing table never reached the task, a table with another layout was
    dropped with its pairs, and a pair's target could be a tombstone.
11. One scenario fixed an order `flush_asking` does not keep, one asserted defect 2,
    and none had two links' messages interleaved.

Of its doubts: an action sent on to a part could arrive before the edge had asked the
part for the chunk; the edge now asks first (section 4). What `hand_over` and section
5 rest on without making sure of it is in section 8.

## Found by the tests written from this record

Sixty-six tests, by someone who had this record, ADR-0013 and ADR-0014 and not the
edge's code: the scenarios of section 9 and A1 to A9, those with two links under every
order of their messages (up to 600 orders each), and generated runs in which regions
merge and split while links are lost, against a second implementation of sections 1 to
6. Sixty-three passed. The three that did not:

1. **Two regions that each have a viewer of a chunk and each name the other for it.**
   A region pinned to where a chunk lies is split, the chunk goes to the part, and a
   player of the pinned region sees it: `Elsewhere` with the part. The part gives the
   chunk back, which makes it the pinned region's again, and nobody tells that region.
   A player of the part sees the chunk: `Elsewhere` with the pinned region. The edge
   was a viewer at both, each told elsewhere with the other; statement E held; nothing
   would ever have either asked again, and nobody served the chunk. This was a gap in
   the records, which the edge had followed: ADR-0014 ends such a ring for players and
   actions and says nothing of subscriptions. A region that is named as the holder
   and has itself said that another holds the chunk is now asked again (ADR-0013,
   section 3).
2. **A step that a stay's new region reported before it had shown the entity was
   passed over.** A presence answer moved a stay to a region; the edge sent the kept
   input; the region applied the move in its next tick and reported it, ahead of that
   tick's snapshots, while the edge still took the entity's moves from the region that
   had shown it last. The view stayed behind until the snapshot came. The records
   were silent; section 2.1, case 3, and section 6 now say whose word counts.
3. **A presence answer that moved a stay had every kept input sent**, also those at
   or below its `last_input`, against case 3. The region passed them over, so nothing
   was applied twice; they are dropped first now.

The regions of the generated runs do not play a pinned region that is behind as in 1,
which the scripted test covers. What the runs cannot reach, and only scripted
scenarios do: a link to a survivor from before a merge whose end the edge has not
read, and a region that forgets the edge around a merge.
