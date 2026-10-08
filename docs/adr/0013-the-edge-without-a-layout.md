# ADR-0013: The edge without a layout

- Status: **Accepted**; the design of the edge's part of step C2b of milestone M3, phase
  C (step C2b.4 of ADR-0012). Revised after an independent review against the code and
  the contract (see the end). Built, and tested from this record by someone who did not
  read the code; what that found is at the end of section 8.
- Date: 2026-10-08

## Context

Until now the edge works out from the layout which region a chunk, a player's position
or a block belongs to: `Layout::region_of` gives it the region players enter the world
in, the chunks a hello names, where it subscribes to a chunk and where it ends that,
where a departed player goes and where a remote action goes, and
`Layout::region_count` how many regions it keeps something for
(`services/edge/src/fanout.rs`).

[ADR-0012](0012-the-tick-on-chunks.md) takes the layout from the sim and the runner: a
region is the chunks the store grants it, and what a region says on a link tells the
edge who holds what. Its section 5 is the contract this record builds the edge against;
"rule n" below is a rule of that section. This record says what the edge keeps and what
it does in each case. It changes nothing a region does, and asks for three lines in the
contract, listed under "Changes to ADR-0012".

What stays as it is: connections, login, the encoding of packets, the routing of
inputs to the player's region, numbered messages and what is kept for a region
(ADR-0008), the resume with its presence answers, the patience with a region, and that
`Fanout` is one task that takes everything in turn. What does not stay is said in
sections 6 and 7.

## Decision

### 1. What the edge keeps

**A port for every region it has heard of**, in a map by region id, made on first use:
when the routing table brings a link, when an entry or an answer names the region, or
when something is to be sent to it. A port without a link keeps numbered messages and
subscriptions as one with a lost link does, so nothing is sent into nothing.

**Per region and chunk, a subscription**, in the region's port:

```rust
struct Subscription {
    /// What the region takes it for.
    kind: Kind,            // Viewer or Guest
    /// The number of the edge's last message on the current link that named the chunk;
    /// 0 for one the hello named, and while the region has no link.
    ask: u64,
    /// The number of the message that made the subscription on the current link, or
    /// asked the region again; 0 for one the hello named.
    begun: u64,
    condition: Condition,  // Waiting, Served, or Elsewhere(RegionId)
    /// How many viewers whose player the edge believes to be this region's see the
    /// chunk. A subscription is a viewer's exactly while this is above 0.
    viewers: u32,
    /// When the region was last asked again for it after a `NotMine`, and whether
    /// another asking is due (section 3).
    asked_again: Option<Instant>,
    again_due: bool,
}
```

and per port the number of the last subscription message sent on its current link
(`asked`).

**Per chunk of the replica**, beside the chunk and the count of all viewers that see it
(`ReplicaChunk::viewers`, as today): `served_by: Option<RegionId>`, the region whose
snapshot the replica has. It is set by a snapshot that is taken and cleared when the
edge has no subscription for the chunk at that region any more or the region has said
`Elsewhere` or `NotMine` for it; it is **not** cleared when a link ends (rule 16).
While it is cleared and viewers remain, the replica keeps the chunk and what clients
were sent; the next snapshot is reconciled with it.

**The home region** is given to the edge by whoever starts it (`Routing::new` takes it
in place of the layout): until step C5 that is the region of the layout that has the
spawn chunk, which the cluster's edge process and the single process both know and
which is the region the store pins as home. `RoutingTable::home` stays unused until
C5.

Three statements hold at the end of every turn of the task. They are checked there in
builds with debug assertions, which is every test run:

- **V**: for every region `R` and chunk `c`, `viewers` of the subscription is the
  number of players of this edge whose `view.region` is `R` and whose `wanted` has `c`;
  there is a subscription with `kind == Viewer` exactly where that number is above 0.
- **G**: a subscription with `kind == Guest` for `c` exists only if some viewer sees
  `c` (`ReplicaChunk::viewers > 0`).
- **E**: for every viewer's subscription to `c` that is `Elsewhere(H)`, there is a
  subscription for `c` at `H`, unless that viewer's subscription has `again_due`.

### 2. A viewer's view changes

`want(region, chunk)` and `unwant(region, chunk)` are the only two ways `viewers`
changes. They are called for every chunk that enters or leaves a player's `wanted`
with the player's region, and for every chunk of a player's view with the old and the
new region when the player's region changes (section 4).

**`want(R, c)`** raises `viewers`. If it was 0:

- no subscription at `R` for `c`: one is made, `Viewer`, `Waiting`, `begun` and `ask`
  the next number, and `Subscribe` is sent (rule 5);
- a guest's subscription there: it becomes a viewer's, `ask` the next number, and
  `Subscribe` is sent (rule 6). Its condition and `begun` stay: a served one stays
  served and gets no answer, and one that waits may be answered under the number it
  began with, if the region had made the answer before the message reached it
  (section 3).

**`unwant(R, c)`** lowers `viewers`. If it reaches 0:

- the subscription is `Elsewhere(H)`: it is ended with `Unsubscribe` and forgotten. It
  carried nothing. If some viewer still sees `c`, every viewer's subscription for `c`
  that is `Elsewhere(R)` is asked again as after a `NotMine` from `R` (section 3): at
  once, or at the next check if it was asked again less than a second ago. Another
  region can have named `R` from a belief older than `R`'s own, and statement E does
  not hold with nothing asked at `R`;
- some viewer still sees `c` (`ReplicaChunk::viewers > 0`): it becomes a guest's,
  `ask` the next number, and `SubscribeAsGuest` is sent, **whatever its condition**.
  A subscription that serves the chunk goes on serving it, and one that waits is
  answered: with a snapshot, or with `NotMine`, which brings the viewers' regions to
  ask again. Ending it instead would leave every other region's viewer that was told
  `Elsewhere` with this region looking at a chunk nobody serves;
- nobody sees `c`: it is ended with `Unsubscribe` and forgotten, and so is **every**
  other subscription for `c`, at every region; the replica forgets the chunk, as
  today.

A guest's subscription is ended in that last case only. One that waits is always
answered, costs a region outside its pinned areas nothing, and is gone with a
`NotMine` or with the last viewer.

Subscription messages are sent in the order the changes happen, each run of chunks of
one kind as one message with its own number, as `subscribe` and `unsubscribe` do
today. There is no gathering per turn: a subscription that is ended and made again
within a turn is an `Unsubscribe` and a `Subscribe`, and begins anew.

**With no link**, nothing is sent and nothing is numbered: `ask` and `begun` are 0, and
the hello of the next link names the subscription.

### 3. Answers

An answer for a chunk the edge has no subscription for at that region is passed over.
Otherwise, with `ask` and `begun` of the subscription:

- **`ChunkSnapshot`** numbered at or above `begun` is taken, whatever the kind is now:
  a snapshot is the region's word that it holds the chunk and serves this link, and a
  change of kind takes nothing of that away. (The region marks a subscription served
  when its tick makes the snapshot and publishes it later; a `Subscribe` or
  `SubscribeAsGuest` that reaches it in between changes the kind of a served
  subscription and is not answered. An edge that took only the current number would
  wait for ever: during a resume's hold, in which the region puts everything behind
  the hello aside, every time.) The subscription is `Served`; the replica takes the
  chunk and its entities, reconciling one it has (section 7); `served_by` becomes this
  region; the chunk goes to the viewers that want it. A snapshot below `begun` is of a
  subscription that was ended, and is passed over. A second snapshot for a served
  subscription is reconciled like the first.
- **`Elsewhere { region: H }`** is taken only at the current `ask` and only for a
  viewer's subscription; any other is passed over. The condition becomes
  `Elsewhere(H)`, also if it was `Served` (which step C3 makes possible); `served_by`
  is cleared if it was this region. If the edge has no subscription at `H` for the
  chunk, it makes a guest's one there, `Waiting`, and sends `SubscribeAsGuest` (rule
  13). `H` being the region that says it is logged as an error and passed over.
- **`NotMine`** is taken only at the current `ask` and only for a guest's
  subscription; any other is passed over. The subscription is forgotten, also if it
  was `Served`; `served_by` is cleared if it was this region. Every viewer's
  subscription to the chunk that is `Elsewhere(this region)` is **asked again**
  (rules 14 and 15): `Subscribe` with the next number, which is its `ask` and its
  `begun`, condition `Waiting`. A subscription is asked again at most once a second:
  if it was asked again less than a second ago, `again_due` is set instead, and the
  check the task makes every second (the one that looks at its patience with regions)
  asks then. `again_due` is dropped when the subscription is ended, is told
  `Elsewhere` anew, or its link ends (the hello asks).

Until its snapshot comes, a chunk that a viewer wants and no region has served is not
shown, as a chunk that is not loaded yet is not shown today.

### 4. A player changes region

Where today `hand_over` finds the new region from the layout, it takes it from the
entry: `Departed { to }`, or the `holder` of a `NotMine` for an arrival (rules 18 and
20). In this order, all in one turn:

1. As today, first: if the edge no longer has the player with the entity of the
   transfer (they left, or left and joined again as another entity), `Discard` goes to
   `to` and nothing else is done. If `to` is the region the entry came from, it is an
   error of that region; the player is disconnected, as today.
2. The player's region has to be the region the entry came from; anything else is
   logged as an error and the entry is passed over (it is about an earlier stay).
3. The view's subscriptions move: `unwant(from, c)` and `want(to, c)` for every chunk
   of the player's `wanted`, with the messages that follow.
4. `view.region` becomes `to`.
5. `PlayerArrive` and the inputs above `transfer.last_input` are sent to `to` as
   numbered messages, as today.
6. The view is centred where the transfer says, as today, which may change `wanted`
   and send more subscription messages.

On the link to `to`, the `Subscribe` of step 3 is before the `PlayerArrive` of step 5,
so that one claim of `to` covers the arrival's chunk and the view. Between the links of
two regions there is no order, and nothing here needs one. A waiting subscription at
the old region is either ended by step 3 or stays as another player's or a guest's;
the `Elsewhere` that follows the `Departed` in that tick (rule 22) is then passed over
for want of a subscription, or taken, and both are right.

**A player is passed on by `NotMine` at most as often as there are ports.** The edge
counts, per player, the hand-overs since a region last confirmed an input of theirs or
placed them; at that many the player is disconnected with "The server lost track of
where you are." and an error is logged. Rule 20 says it cannot happen while beliefs
form no ring; step C3 has a case in which they can.

A player who joins is the home region's from the join on; one who leaves or is
removed is `unwant`ed everywhere.

### 5. Blocks

- `Remote { action, to: Some(r) }` and `NotMine { what: Remote(action), holder: r }`
  are passed to `r` as `EdgeToWorker::Remote`, numbered, as a remote action is today;
  `r` being the region the entry came from ends the action at the edge, as today.
- `Remote { action, to: None }` goes to `served_by` of the chunk of
  `action.step.concerns()`. If there is none, or it is the region the entry came from,
  the action ends at the edge as today (`Fanout::arrived`); if the edge no longer has
  the player, the entry is dropped (rule 26).

### 6. Links

**Where a hello is made** (`take_link`, whether the link before ended or is replaced
while it still stands) and where a link is found lost (`lose_link`, a failed send):
every subscription of the port keeps its kind and its viewers; `ask`, `begun` and the
port's `asked` become 0, its condition `Waiting`, `again_due` false. `served_by`
stays. The hello names the viewer's subscriptions as `chunks` and the guest's as
`guests` (rule 2), **those told elsewhere included**: the region may have been
restored and asks the store again. Answers come under the number 0.

**The welcome's entries come before what was kept** (rule 3): the link counts the
`Outbox` messages after its welcome, and sends what the port kept when `entries` of
them have been handled, at once if `entries` is 0. Until now it sent everything at the
welcome.

When a region answers `Unknown` and the edge gives up what it kept for it (ADR-0008,
section 5), the players of that region are removed, which `unwant`s their chunks by
section 2: what other regions' viewers still see stays, as guests'.

### 7. Entities

An edge takes what is said of an entity only from the region that last introduced it
(`Shown::from`, ADR-0008). One thing changes: **a snapshot that is reconciled removes
only entities that this region introduced.** Until now only one region could say
anything of a chunk, and `take_snapshot` removes every entity shown in the chunk that
the snapshot lacks, the edge's own players excepted. Now a player of one region can
stand in a chunk another holds while the store answers (rule 32); the holder's
snapshot lacks them, and their moves come from their own region.

### 8. Building it

The edge has to work against regions that presume and regions that ask the store
(ADR-0012, section 8); both are run end to end from step C2b.3 on.

| # | Scope | Its tests |
|---|---|---|
| E1 | Ports in a map, made on first use; the welcome's entries before what was kept; a reconciled snapshot removes only what its region introduced. Subscriptions still by the layout (built) | The edge's unit tests as they are; below, 18 and 19 |
| E2 | The subscription table, `want` and `unwant`, guests, answers by number, `served_by`, the hello's two lists, links, and the view's subscriptions moving with a player who is handed over (built) | Below, 1 to 12 and 20; statements V, G and E at the end of every turn |
| E3 | `Departed { to }`, `NotMine` for an arrival with its count, `Remote` by `to` and by `served_by`; the layout goes from the edge, which is handed the home region in its place (built) | Below, 13 to 17 and 21 |

**Existing tests whose point changes** (`fanout.rs`):
`a_lost_link_keeps_the_players_and_a_new_one_resumes` asserts that the hello names
"every chunk of the region ... and none of the other region's"; it names what the
region's players see, whoever serves it, and the guests'.
`a_remote_action_is_passed_on_to_the_region_that_has_the_block` goes by `to` and by
`served_by`. `a_departure_missed_with_the_link_hands_the_player_on_before_presence_is_judged`
is the hand-over read from a welcome's entries, with its subscriptions.
`a_region_that_forgot_the_edge_gets_nothing_that_was_kept` also sees `Unsubscribe`s.
The `Harness` gets a third region.

Scenarios for whoever writes tests from this record alone, on scripted regions:

1. A player joins: the home region is sent `Subscribe` for every chunk of the view,
   and no other region anything.
2. `Elsewhere { region: H }` makes one `SubscribeAsGuest` to `H`; a second
   `Elsewhere` naming `H` for the chunk, from a third region's viewer's subscription,
   makes none.
3. A snapshot numbered at or above the number the subscription began with is shown,
   also when a later message has changed the subscription's kind; one below it, or
   from a region the edge has no subscription at, is not.
4. **A kind change with a snapshot in flight.** The edge is a guest at `H`, waiting;
   `H`'s snapshot is made; a player is handed to `H` and the edge says `Subscribe`;
   then the snapshot, with the guest's number, is read: it is shown, and nothing more
   is asked.
5. **A hand-over into a region during its hold.** The same with a link to `H` that has
   just said hello: the answers numbered 0 are taken although `Subscribe` 1 was sent
   meanwhile.
6. A chunk leaves a view: `Unsubscribe` to every region the edge has a subscription
   for it at, if nobody else sees it.
7. Two players of two regions see one chunk that the first one's region serves. The
   second region is told `Subscribe` and answers `Elsewhere`. The first player leaves:
   their region is sent `SubscribeAsGuest`, the chunk stays shown without a new
   snapshot, and block events for it are still shown to the second.
8. **The same while the first region's link is down**: the first player is removed;
   the hello of the next link names the chunk among `guests`, and its snapshot is
   shown to the second player.
9. `NotMine` from a guest's region makes `Subscribe` again at the viewer's region
   under a new number; a second `NotMine` within a second makes the next asking only
   when the task's check comes.
10. `Elsewhere` at an old number, `NotMine` for a viewer's subscription, `Elsewhere`
    for a guest's and `Elsewhere` naming the region that says it change nothing.
11. **A link replaced without ending**, and one that ended: the hello's `chunks` and
    `guests` are the subscriptions by kind, those told elsewhere among them; answers
    numbered 0 are taken; the chunk stays shown meanwhile.
12. A region answers `Unknown`: its players are gone; what a player of another region
    sees of its chunks stays subscribed as a guest's.
13. `Departed { to }`: on the link to `to` the `Subscribe` for the view comes before
    `PlayerArrive`; the old region is sent `SubscribeAsGuest` for what others still
    see and `Unsubscribe` for the rest.
14. **A hand-over read from a welcome's entries**, with a second viewer of the old
    region's chunks: nothing stays without a subscription that someone sees.
15. `NotMine` for an arrival is handled as a `Departed` to its holder, also back to
    the region the player came from; passed on as often as there are regions, the
    player is disconnected.
16. A `Departed` or a `NotMine` for a player who left, and for one who left and joined
    again: `Discard`, and the one who joined again is where they joined.
17. `Remote` with a region goes there; without one, to the region that serves the
    chunk, also while that region's link is down; without one and served by the region
    it came from, or by nobody, it is acknowledged to the player. A region the edge
    has no link to yet is kept for.
18. The welcome announces two entries: what was kept is sent after the second.
19. A reconciled snapshot that lacks an entity another region introduced leaves it.
20. Statements V, G and E hold after every step of a random sequence of joins, moves,
    hand-overs, answers, lost and replaced links, for three regions and four players.
21. The end-to-end tests of hand-over, blocks, takeover, chaos and moves, with regions
    that presume and with regions that ask.

**Found while building E2.** The three statements are checked at the end of a turn that
changed a subscription or who sees what: in the edge's own tests every such turn, in
other builds with debug assertions ten times a second at most. Checked after every
message, an edge with four viewers at a view distance of 8 did nothing else: going
through every subscription takes longer than it has between two messages, a resume
alone being hundreds of answers, and its players were disconnected for the wait. The
subscription messages of a turn are gathered by region and kind where no chunk is
named twice for a region, so that a view that moves is three messages and not one for
every chunk; where one is named twice, they go in the order they were made.

**Found by the tests written from this record** (59, by someone who did not read the
edge's code; scenarios 1 to 20 and runs generated against a second implementation of
sections 2 to 6):

- Section 2 contradicted statement E. A viewer's subscription that was told elsewhere
  was ended with its last viewer as one that carried nothing, but another region's
  subscription could be told elsewhere with *its* region: the north names the east
  from an older belief, the east names the west, the east's player goes, and the
  north's subscription pointed at a region where nothing was asked, never to be asked
  again. The edge did as the record said and then failed its own check. Those
  subscriptions are now asked again, as the first case of `unwant` says. Pinned
  stripes cannot produce such a belief; regions that merge and split can.
- A discarded entity stayed on screens when another region had shown it last. A player
  is let go to the east and leaves; the east takes the arrival in before the leave,
  shows the entity, and lets it go to the west within the same tick. The edge has the
  west discard it, and the west reports it removed, but section 7 takes a removal only
  from the region that introduced the entity last, the east. The edge now takes an
  entity it has discarded off its own screens itself. That also covers an edge that is
  not asking the discarding region for the chunk and would never hear the removal.

The tests read the letter of the record where it was silent, and the edge agrees with
each reading: a player is passed on by `NotMine` as often as there are ports and
disconnected at the next; `served_by` stays through an `Unknown`; a hand-over leaves
the old region a guest's subscription for the whole view the player still sees, which
costs a `NotMine` at regions that do not hold those chunks.

## Consequences

- The edge no longer knows how the world is divided; step C5 can take the layout away
  from the rest.
- A chunk another region holds takes a round more to appear the first time: the
  viewer's region answers `Elsewhere`, and the holder then sends the snapshot.
- The edge holds more per chunk: a subscription for each region that has a viewer of
  it, and guests'. A region goes on being asked as a guest for a chunk as long as
  anyone sees it, also when it will answer `NotMine`.

## Changes to ADR-0012

- **Rules 8 and 9**: an edge takes a `ChunkSnapshot` numbered at or above the number
  its subscription began with, not only at its last message's number. A message that
  changes the kind of a subscription the region has already served is not answered,
  and the snapshot that is in flight carries the number from before.
- **Rules 5 and 6**: a viewer's subscription that loses its last viewer while someone
  still sees the chunk becomes a guest's whatever its condition, unless it was told
  elsewhere; it is not ended because it waits. A guest's subscription is thus also
  made from a waiting viewer's.
- **Rule 3** is met by the edge from step E1 on.

## Open questions

1. Whether a second between two askings of one subscription is right; a world of
   pinned regions never comes to it.
2. With a second edge, entities that a region introduced on one edge's behalf are
   still an open point of ADR-0008; section 7 closes the one case that pinned regions
   with one edge do not hide.

## Review

An independent review against the code and the contract found nineteen defects in the
first version of this record, all worked in above:

1. A change of kind on a subscription the edge held as waiting lost the snapshot in
   flight, because only an answer with the last number was taken; during a resume's
   hold, every time.
2. Ending a viewer's subscription that waited left every other region's viewer that
   was told `Elsewhere` with this region looking at a chunk nobody served, for good:
   after a lost link, a hand-over read from a welcome's entries, or `Unknown`.
3. What was "left of several changes within a turn" was not defined, and with the
   tidying of guests in between turned an ended and remade subscription into a lone
   `Subscribe`.
4. The reset of subscriptions hung on "a link ends", and a link is replaced without
   ending.
5. The statement about guests was false by the record's own rule for lost links.
6. `served_by` was cleared while the chunk stayed on screens.
7. A hand-over moved a player's subscriptions before it was known that the edge still
   had that player with that entity.
8. "Five ticks of the edge" had no clock and no field.
9. Regions without a port: numbered messages to them were dropped without a word.
10. The first step could not hold the statements it was to check.
11. Scenarios that could not be written at their step, or missed the cases found here.
12. Nobody filled the home region of the routing table.
13. Reconciling a snapshot removed entities another region had introduced.
14. Rule 3 of the contract was not met, in a part said to stay as it was.
15. Answers the record gave no case for.
16. Nothing bounded a player passed on by `NotMine`.
17. Claims about the code that were not so (where the layout is used; why an
    `Elsewhere` after a `Departed` is passed over).
18. Existing tests whose point changes were not named.
19. "To `to` first" ordered messages on two links, between which there is no order.

The reviewer found the bookkeeping of viewers, the end of every guest's subscription,
the handing over and back of two players who see one chunk, `NotMine` for an arrival
and the routing of block actions sound.
