# Faces: how the plan's 7.3, 8.3, 9.2 and 10.4–10.6 are built (N7a2), as reviewed

Revised after the review of 2026-10-19 (findings R1–R7 below, all taken).

## What a face is

`clustine_sim::Face` (api.rs, serde, `PartialEq`), what a mob reads of a player it does not
hold: `position: [f64; 3]` (feet), `yaw`, `pitch`, `eye_height: f32`, `main_hand`,
`off_hand`, `worn: [Option<ItemStack>; 4]`, `sneaking`, `game_mode: u8`, `sleeping:
Option<u32>`, `vehicle: Option<EntityId>`, `alive: bool`, `last_hurt_mob`, `last_hurt_by`
(`Option<(EntityId, u64)>`, game time), `moved_horizontally: bool`. Derived from a `Player`
or a `Transfer`.

A face's sequence `FaceSeq(u64, u32)`: the commit's log sequence and the note's index in
the commit. Log sequences are durable and only rise, so this is the plan's (group, index).

## Commit 1: the sim's and the rpc's contracts

1. `StayNote::Has` gains `face: Face` and `let_go: bool`. `StayNote::Gone { player, entity,
   hops }` is written for a leave of the living stay, a `Discard` (below), and when a
   region drops a `Departed` because its edge has gone (R1). Not for a dead stay or a
   replacement (the store's floor already removes the face: R, question 3). **A player's
   `Gone` is that player's last note in its commit** (R2: a rider who quits in a boat has
   the dismount's `Has` and then the `Gone`).
2. `PlayerChange::Discard` gains `player` and `hops`. A region writes `Gone` for every
   `Discard` unless it holds a stay of that player at or above it (R1); the store judges.
3. `TickInputs.faces: Vec<FaceChange>` in stream order, each with the stay `(entity, hops)`:
   `Face { player, entity, hops, seq, face }`, `Removed { player, entity, hops, seq }`.
4. The region keeps in memory (not state; remade after a restore, R5):
   - `faces: BTreeMap<PlayerId, Kept { entity, hops, seq, face, departed: Option<u64> }>`
     (departed: the tick of the let-go);
   - `tombstones: BTreeMap<PlayerId, FaceSeq>` (the highest);
   - `known: BTreeMap<PlayerId, (EntityId, u32)>`, the highest stay it has held or let go
     (R3).
   A face is taken where its `(entity, hops)` is at least `known`'s, its sequence at least
   the tombstone's and above the kept face's, and the player is not held. A removal at or
   above `known` drops a face strictly below its sequence and is kept as the tombstone if
   higher.
5. **What mobs see is held and copied land only** (R4, as built): every face so taken is
   kept, but the view lists only those whose chunk the region holds or copies (and the
   departed ones). The worker's borders can hold a copy's base back until its chunk may
   be read, and faces streamed meanwhile would otherwise be refused and lost. A face
   kept beyond what the region holds and copies is told of no more and may be out of
   date, so every base of faces begins with `FaceChange::Base { chunks, seq }`: what the
   region keeps in those chunks (every chunk, for `None`) at or below `seq` is dropped
   before the base's faces are taken.
6. **The departed face** (8.3.1, 9.2.1, R3): at a let-go the region keeps the player's face
   as departed, with `known` raised to the let-go's `(entity, hops)`. It ends at the first
   of a face of that player with `(entity, hops)` at least the departure's, a removal at or
   above it, the player taken in, or 100 ticks.
7. `TickInputs.world_players: Option<Vec<WorldPlayer { player, entity, position,
   game_mode, sleeping }>>`, the store's publication; the region keeps the last and the
   view answers from it with its own players in place of their entries.
8. The view's `players()`: held players, then kept faces ascending by player.

## Commit 2: the store

1. A `Record` gains, in memory only: `face: Option<Kept { face, entity, hops, seq, chunk }>`,
   `writer: Option<RegionId>`, `let_go: bool` (the latest taken note was a let-go), and an
   index `faces_by_chunk`. The players' file and `LoggedPlace` are unchanged; the logged
   note (`LoggedStay`) gains `Gone` and `let_go` but not the face (a start replays notes
   through `apply`; faces come back from every region's first tick, which names every stay).
   Plan 8.3.2's "the file marks the record offline" is met by the record having no face in
   memory; the file needs nothing (a named departure from the plan's wording).
2. A taken `Has` of the living stay from region W: the face is replaced, `writer = W`,
   `let_go` from the note. Its audience: the chunk's copiers and holder, less W. If the chunk
   changed, the old audience less the new audience less W is sent a removal at the same
   sequence (R3: never to the new writer; the old writer out of the old audience).
3. A counted `Gone`: the face goes, a removal to its audience, out of the world list; the
   same at an `Entering` that raises the floor.
4. All on the group's streams (`Streamed::Reply`), out once durable, in the group's undo.
5. **Bases** (R5, R7): behind a `CopyBase`, `StoreReply::Faces` with the base marker and
   its chunk's faces as the records have them when the base is streamed; behind a
   grant's answer, those of the chunks granted; behind a merge's, every chunk the
   survivor holds; a hello's answer (`Restored::faces`, in the welcome) has the faces of
   every chunk the region holds. A split's part is opened with a hello. A lend's take-in
   gets none (named: the borrower sees players in a lent chunk only once they move).
6. The world's players with the clock every 20 ticks of game time, to every owner.
7. At an `AbsorbCommit` the records whose writer is the absorbed region are rewritten to
   the survivor (R6).

## Commit 3: `Gone`'s rule and the offline marks

1. A `Gone` counts from the writer with `hops` at least the record's, from any region with
   `hops` above it, and from any region with `hops` at least the record's while the
   record's latest note was a let-go (R1). A later `Has` of the living stay brings the
   player back.
2. Offline marks (9.2.4, 10.5) as the plan has them; a `Gone` is a note that names
   nobody for the mark. After a store restart every region's first tick marks again.
3. ADR-0020: the `Gone` row; row 17 (a `Discard`: now `Gone`); section 14 the fields.

## Tests

Sim: the taking rules (sequence, tombstone, `known`, held player, holdings); a removal
below `known` refused; the invariant at each kind of shrink; the departed face's ends,
and not ended by a late removal below the departure (R3a); a let-go face arriving after
the player came and went is refused (R3b); `Gone` is last; `Gone` for a discard of a
stay the region never held. Store: the audience and removals with W left out; a move
between chunks; a counted and an uncounted `Gone` (each of R1's cases, a leave in the
hand-over gap, an edge gone); a merge's writer rewritten and a leave in the survivor's
first tick; bases at a copy, a hello, a merge, a split and a grant; nothing streamed
before durable and an undone group takes faces back; the world list with the clock.
End to end: a test region's mob view of a player across a pinned line through the
worker; twenty sheep tempted across a line in N7g.
