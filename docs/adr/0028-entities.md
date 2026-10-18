# ADR-0028: Entities other than players: keys, chunks, commits and crossing

- Status: **Accepted** on 2026-10-18, from the plan [n-plan.md](../groundwork/n-plan.md),
  revised after two reviews and agreed with the owner's answers on 2026-10-17 (its
  section 6). Four reviews of this record against the code found it not ready and a fifth
  ready after small fixes; their findings are worked in ("Review").
- Date: 2026-10-17. The code it cites is as at `98e8311`.
- It amends ADR-0022 (a chunk carries its entities), ADR-0024 section 6a (entities in the
  tick), ADR-0025 sections 1 and 5 (arrivals are not effects and are never dropped),
  ADR-0008 and ADR-0014 (a region's block of keys is fresh at each hello, and a split's
  part has one), ADR-0020 section 7 (the store's rule that the home region's block lies
  above the highest stay, and `Restored::issued`, go: blocks rise by construction) and
  ADR-0015 (`Shown::from` is kept only for players and equal `hops`), and answers ADR-0006's open question on the versions of entity states (`hops`); ADR-0010's
  "when things other than players need ids away from home, that has to be decided" is
  decided here (section 1).
- Marks as in the plan: `file:line` read; a class and method of the game read in the
  26.3 jar; **Inferred**, reasoning to test.

## Context

The only entities are players (`EntityKind::Player`, `clustine-sim/src/api.rs:227`).
Their ids are `EntityId(i32)` from blocks of 2^20 (`clustine-world/src/position.rs:87-165`)
that the store gives a pinned or home region when it is first opened and that the region
keeps (`worldstore/src/lanes.rs:2363-2392`); a split's part gets none
(`clustine-sim/src/region/reshape.rs:90-96`). The edge shows an entity under that id,
taking moves only from the region that introduced it last (`edge/src/fanout.rs:790-805`,
`2698-2703`). A chunk carries no entities (ADR-0022), the log has none (`clustine-format/src/log.rs`),
and what crosses a line between regions is either a player's hand-over through the edge
(ADR-0006, ADR-0020) or an effect addressed to a block (ADR-0025). The plan says what a
player meets for want of entities, and what the game does; this record decides how
entities live in a world of several regions. What each entity type does is the game's,
ported step by step (the plan, section 4).

## Decision

### 1. Keys, the game's id and the wire's id

**Keys.** `EntityId` becomes a `u64`, the entity's **key**, unique for the life of the
world. `EntityIds` blocks hold 2^32 keys each (`BLOCK_SIZE = 1 << 32`), and the key's
high half is the block's index, so blocks never meet; no block holds key 0.

- **Every hello is given a fresh block.** The store gives the region opened, pinned or
  not, the next block at each hello and writes it to the region file before it answers,
  as it writes the first block now. A restore sets the state's next key to the fresh
  block's first, whatever key the checkpoint and commits had reached. A worker
  publishes a tick only once it is durable (`worker/src/lib.rs:2236-2238`), but a
  durable write can still be lost with its machine (the write-back cache, CLAUDE.md):
  then the edge has seen keys that the restored state gives out again. With a fresh
  block they are never given to another entity.
- **A region half way through its block** names that once in a commit
  (`TickOutput::keys_low`; naming it again in later commits gets nothing more) and the
  store gives it the next block on its stream. A
  region whose block runs out before then spawns nothing further until it comes (an
  item that would have been made is not, as the game drops an entity it cannot add).
  **Inferred**: at 2^31 keys before the ask, no region meets that.
- **A split's part** is opened with a hello before it runs
  (`bin/clustine/src/cluster/worker.rs:967-990`), and takes its block from that hello's
  `Restored::entity_ids` (`of_part`, `worker/src/lib.rs:1156-1176`, which now ignores it,
  and `reshape.rs:323`, which makes the part with no block).
- **Stays keep their order** by key. Blocks are given in increasing index, so the home
  region's fresh block is above every stay ever given, which the store's rule at
  `lanes.rs:2367-2378` ensured for the old blocks, and which now holds by construction;
  the rule goes.

**`next_block` is kept in a synced file of its own** (`regions/blocks`), raised and
synced before any block is given, at a hello or on a stream, instead of being worked
out at the store's start from the highest block a region file names
(`lanes.rs:361-367`). Every region now has a block, so the files of absorbed regions,
which are kept while they name one (`absorb`, `align`), are kept for good: in a world
whose regions follow players they pile up, one small file for every part ever opened,
and they let a store whose blocks' file is lost start above every block given. Clearing
them is left for later. The region file still names its block, for its
restore, and the store's start takes the highest of `regions/blocks` and the blocks the
region files name, against a damaged file. A hello raises `next_block` by sixteen blocks
at a time and keeps the rest for the next fifteen hellos, so that most hellos sync only
the region file. With a fresh block at each hello, a hello of the same epoch (after a
store restart) rewrites and syncs the region file where it did not. Format 3 of the store's files
writes keys as `u64`; the store opens no world of format 2, as it refused format 1
(`3c9990c`): a world made before N1 is made again.

**The game's id.** `Entity.getId()` is game logic: an item at rest moves on the ticks
where `(tickCount + getId()) % 4 == 0` (`ItemEntity.tick`), and a mob re-evaluates its
goals by `tickCount + getId()` (`Mob.serverAiStep`). The sim's `EntityId::game_id()` is the
key's low 32 bits as an `i32`, used wherever the game uses `getId()`.

**The UUID** of an entity is made from the world's seed and its key by the stream of
section 4, version 4 as the game's (`Mth.createInsecureUUID`), saved with the entity and
never used to identify it inside the server. A player's UUID is their profile's.

**The wire's id** is the edge's. Each edge keeps a map from keys to `i32` wire ids, given
out in increasing order from 1, and the reverse map:

- a key is given a wire id when the edge first shows its entity to a connection, or
  when a packet must name it (below);
- a wire id is freed when no connection of the edge shows the entity and no connected
  player of the edge is that entity, and then kept from reuse until every connection
  that showed it has answered a ping (`Ping`/`Pong` of the play state) sent after its
  removal: an interact that a client sent before it took the removal must not reach
  another entity that has the id by then. Until something a client sends names an
  entity (N7's interact), N1 gives no wire id twice in an edge's life and only forgets
  the map's entry; the quarantine and reuse come with the interact;
- a player's own id, sent at login, is their stay's key's wire id, kept while they are
  connected; a new stay of the same player (a join after a leave elsewhere, ADR-0020) is
  a new key and a new wire id, as it is a new entity id today;
- **every id in a packet is translated**: clientbound the entity's own, `SetPassengers`,
  `SetEntityLink`, `TakeItemEntity` (the item and the collector), `DamageEvent` (the
  source and the direct source, written as id + 1 with 0 for none), `SpawnEntity`'s data where it is an owner's id
  (projectiles, fishing hooks, ...), and entity data fields that hold ids (a fishing
  hook's hooked entity, a firework's attached entity, a guardian's target, ...), which
  the protocol crate names with their types; serverbound interact, pick-item-from-entity
  and any other that names an entity, from wire id to key. A wire id the edge does not
  know is passed over: the action names nothing.

So the 2^31 ids of a world spawning items and mobs all day are no limit, and no region
but the home region issues stays, as before.

### 2. An entity is part of its chunk

An entity belongs to the chunk its position is in, by the game's rule
(`SectionPos.blockToSectionCoord` of its x and z, so `floor`), and is part of that
chunk's state: `Chunk::entities`, a list ordered by key of `Entity` values (key, type,
UUID, position, motion, rotations, on-ground, fall distance, fire, air, passengers'
keys, vehicle's key, the type's own fields, and its unknown fields kept as NBT bytes, as
a block entity's data is). It is not part of the region's opaque state, because the
store replays chunks' changes at a restore (`clustine-rpc/src/messages.rs:743-745`) and
makes copies' bases from commits (`worldstore/src/chunks.rs:995-1030`): both read
entities by chunk.

- **Stored apart from the blocks.** A chunk's entities are a record of their own in the
  store, `chunks/overworld/{x>>5}.{z>>5}/{x}.{z}.entities`, beside the file of its blocks, as the game keeps `entities/` apart from
  its region files (`EntityStorage`): a mob's step marks only the entities dirty, and a
  merge's or a split's saves of chunks, which are of blocks, do not grow with it. A chunk
  with no entities has no record.
- **With the chunk.** A merge takes the absorbed region's entities with its chunks; a
  split gives the part those of its chunks; a lend lends them; a return takes them with
  it as at the return's commit; a move of a region moves them with its chunks. Nothing
  in any of these is decided per entity.
- **A chunk no region holds** keeps its entities frozen in the store, as the game keeps
  them in a chunk that is not loaded.

**Which entities tick.** The game ticks an entity where `inEntityTickingRange` holds for
its chunk (`ServerLevel.tick`), one ring inside the chunks that take random ticks. Here an
entity ticks where its chunk's 5x5 is loaded or copied in its region, and stays as it is
elsewhere, as the game leaves an entity in a chunk that is loaded but not ticking.

**Order in the tick** (added to ADR-0024 section 6a, as `ServerLevel.tick` runs): after
block events, **the entities**, then block entities. Arrivals (section 5) are taken
before the entities tick, with the tick's effects. Entities tick in ascending key; a
passenger ticks with its root vehicle (`Entity.rideTick`), not by its own key. An entity
removed in the tick is not ticked after its removal; one added before the entity phase
(a throw, a drop, a block starting to fall) ticks in that tick, as the game's does, and
one added during the entity phase in the next (amended by the plan of N4, section 2). Lookups of entities in a box go by chunk, then section, then key, so
that what an entity finds is a function of the state.

### 3. Every tick's entity changes are in the commit

The owner's answer to the plan's question 1: a crash loses no tick of any entity. The
log records results, so each commit carries the tick's **entity changes**, as
`Change`s keyed by chunk and key, beside the block changes:

- `EntityChange::Whole { chunk, entity }`: the entity as it is after the tick, where it
  is new to the chunk (spawned, arrived, moved in from another chunk of the region, or
  taken in from the store's record at a load, which a lend's borrower does too);
- `EntityChange::Gone { chunk, key }`: it left the chunk (died, picked up, sent, moved
  out to another chunk of the region);
- `EntityChange::Set { chunk, key, fields }`: only the fields the tick changed, each to a
  definite value.

A move between two chunks of a region is `Gone` in one and `Whole` in the other, so that
a copy of the second chunk is never sent fields of an entity it does not have. Replay
sets definite values and is idempotent, as block replay is (`chunks.rs:1460-1466`); a
`Set` of a key the chunk does not have is passed over. An
entity change counts as its chunk's change for a copy's gap check (ADR-0025, section 3).
A checkpoint writes each chunk's entities whole, in its entity record; a chunk whose last
entity has gone has its record deleted, behind any write of it still pending. A copy's
base reads a chunk's blocks and entities as of one sequence. The log's new kind of
commit is named apart from `KIND_COMMIT_WITH_ENTITIES` (`log.rs:115`), which means block
entities.

To keep at rest what is at rest, so that an item lying on the ground or an animal
standing commits nothing:

- **counters are derived**: a birth tick rather than an age, a tick "until" rather than a
  countdown, wherever the game counts by one a tick; the type's code reads them as the
  game's fields;
- **randomness has no state** (section 4).

The plan's estimate, about 40 bytes a tick for an entity that moves, is measured in N2
for items and in N7 for mobs with AI, as R8 measured circuits.

**Copies** (ADR-0025, section 3) carry entities the same way: a copy's base has its
chunk's entities, its updates the entity changes of later commits. A region reads the
entities in its copies where it reads its own (what an entity sees and collides with
across a line), never ticks or writes them.

### 4. Randomness

An entity draws from a stream counted from (world seed, key, the tick's number, the draw's
index in the entity's tick), with no state to keep. Spawning draws from its chunk's
stream, generation's spawning from the game's own `WorldgenRandom` with
`setDecorationSeed(worldSeed, x, z)` (`NaturalSpawner.spawnMobsForChunkGeneration`), so
that which animals a chunk is made with is the game's exactly. The game's entity randoms
are seeded from the clock (`RandomSource.create()` in `Entity.<init>`), and the owner's
answer to question 5 makes parity for what they draw the game's rules and distributions.

### 5. Crossing a line: arrivals

An entity whose position comes, in a tick, into a chunk its region does not have loaded
(another region's; its own but not loaded, or asked for and not yet loaded; a chunk it
is lending), is **sent**: its chunk's commit has it `Gone`, and the same commit writes an
**arrival**: the entity whole, addressed to the chunk, with the `hops` it had. The store
files an arrival as it files an effect (ADR-0025, section 2), numbered by its group's
sequence and its index (an `EffectId`), kept apart from effects (an effect is small and
`Copy`, an arrival is not), and **by the chunk's holder** in the table
(`Table::holder`, a grant over a pin), **not by its use**: effects are sent again by use
(`lanes.rs:2496-2497`, `2596`, `2648`, `2772`), which ends at `Dropped`, and an arrival
must reach a holder that does not use the chunk. At a hello the store sends the region
every arrival filed for chunks it holds; at a merge or a split, those of the chunks whose
holder the change makes another region go with them.
Commit and arrival are one group, so the entity is in exactly one place in every durable
state.

- **Taken once, as effects are** (ADR-0025, section 5, "taken as a set"): the taking
  commit names the arrivals it took by number, the store drops them in that group, and a
  worker passes over an arrival resent after it took it. The taking commit has the entity
  `Whole` with its `hops` one higher (section 6). A key the holder has already is a
  malformed stream, refused as one, not the rule that keeps an arrival from being taken
  twice: the entity may have been taken, sent on and be elsewhere by then.
- **When.** The holder takes an arrival in the tick that takes the effects of its group,
  before its entities tick, so that it ticks in that tick. An arrival is ready once its
  chunk is loaded; it needs no copies of the 3x3. Arrivals are never deduplicated by
  key, only by number (below).
- **While coupled** (ADR-0025, section 8), the holder runs border time t + 1 only once
  the sender is durable through t, and takes the arrival in it: no tick of the entity is
  lost. Uncoupled, it is taken one to a few ticks late, standing still meanwhile.
- **Every arrival has a holder; the store never writes one into a stored chunk.** An
  arrival filed for a chunk keeps that chunk held, so that what holds the entity is
  always a region, whose commits and restore are the one way an entity's state moves:
  - **into land held by no region** (only in a world whose regions follow players: a
    pinned division holds all land, `Division::side_by_side`, `table.rs:71`, `103`), the
    store grants the chunk to the sending region in the arrival's group, as it grants a
    claim of land nobody holds (`lanes.rs:1299-1323`): the `Granted` record is written
    before the commit's record in the group, so that a restart that replays the log, or
    keeps only the first part of a torn group (`log.rs:90-92`), files the arrival against
    its holder; the grant goes into the group's changes and the filing into its undo, so
    that a cut group takes both back; the commit is checked before its `Granted` record is
    appended, so that a refused commit leaves no grant behind. The store answers
    `Claimed { granted }` as a reply in the group (`answer_in_group`), which reaches the
    worker ahead of the group's stream and so ahead of the arrival, as a lend answers for held chunks
    (`lanes.rs:2002-2009`), which is how the sim comes to hold the chunk
    (`inputs.granted`, `region.rs:2120-2127`), and at a split the answer goes by nearness
    as a claim's does (ADR-0017, section 3.6.1). The sender loads the chunk with an
    arrival ticket (below) and takes its own entity in. A chunk held only for an arrival
    is given back as soon as its arrival ticket ends and the arrival is taken, not after
    `return_after`, so that a pearl or an entity launched into land next to another region
    does not keep that region's players out of it for half a minute (the pattern ADR-0017
    removed). This path is rare: entities tick only where their 5x5 is loaded or copied,
    and copies are not taken beside ring chunks (ADR-0025, section 3);
  - **a `Return`** of a chunk with arrivals filed, where the release would leave no
    region holding it (no pinned holder): `Return` is not answered, and the region
    forgets the chunk when it sends it (`messages.rs:370-376`, `region.rs:2279-2281`).
    So the store leaves the chunk out of `returning`, answers `Claimed { granted }` for
    it, and sends its arrivals again; the sim holds the chunk again, and the arrival
    ticket loads its save. That is so for arrivals already filed when the `Return`
    comes (the `Return`'s handler, `lanes.rs:1349-1406`; `end_uses` there drops no
    arrival) and for those filed while the chunk is being returned, which call the return
    off as a claim does (`lanes.rs:1287-1294`). A `Return` where the release leaves a
    holder (a borrower giving a lent chunk back, `region.rs:2277-2278`) is the next
    case;
  - **a lent chunk given back** (`returned`, `lanes.rs:2232-2248`, then `PinnedBack`)
    passes its filed arrivals to the holder the release leaves (`Table::holder`), on
    that region's stream, behind `PinnedBack`, which is a reply of the same group. After
    `Lent`, the lender drops the arrivals it was sent for the chunk, as it drops effects
    (`Border::drop_effects_of`, ADR-0027 step 4), and the store files them for the
    borrower with the grant;
  - **a return or a lend called off** (a claim that calls off a return, `LendCalledOff`):
    the store sends again the arrivals filed for each chunk taken up again, since its
    holder did not change and nothing else would;
  - **a world made over for another division** folds the old regions' commits into the
    stored chunks at the store's start (`chunks.rs:1195`, `lib.rs:405-411`), entity
    changes into the entity records among them, before anything runs; the arrivals left
    untaken are written into their chunks' entity records in that same fold, whole by
    key, which is the one place a stored chunk takes an arrival, with no region running to
    race it. Since every load files arrivals again from every commit still in the log
    (`lanes.rs:465-486`), the fold writes a durable dropping record for each arrival it
    wrote, before the `Opened` records are synced, and clears them from what is filed;
    a fold done again after a crash writes the same entities by key. The chunks of
    untaken arrivals are among those the fold loads (`touched`); an arrival whose chunk
    the fold cannot load or make stays filed, under the next rule;
  - **any grant of a chunk with arrivals filed** (a claim's, a lend's) puts them on the
    new holder's stream, so that an arrival left filed for land nobody holds (after a
    fold that could not load its chunk, or an unreadable chunk given back, below) reaches
    whoever comes to hold the chunk.
- **Held and not loaded.** A chunk an entity enters can be held by a region that does not
  use it (a pinned region's land beyond its viewers). The arrival goes on the holder's
  stream all the same, and the holder loads the chunk for it with an **arrival ticket**
  (as a lend's ticket, ADR-0027, with no ticking of its own), which lasts until the chunk
  is loaded and the arrival taken, however long generation takes, and then 100 ticks;
  the chunk goes with its next save when the ticket ends. No arrival ticket loads a chunk
  the region is lending or giving back. A chunk that cannot be read (`Unreadable`) holds
  no ticket: the worker keeps its arrival, a `Return` of it is allowed whatever arrivals
  are filed, and they stay filed for whoever is granted the chunk next.
- **What a worker keeps.** A worker keeps arrivals only for chunks it holds or is about
  to hold, as the **runner** judges from the store's replies it has received (a chunk
  granted by a `Claimed` not yet given to a tick, a `Lent` or `PinnedBack` to come), not
  as the sim does, which holds a granted chunk only from the tick the grant is given to;
  the arrival goes to that same tick. It keeps them **by number**, so that an arrival sent again (at a
  hello, a merge, a split, a call-off) while the worker still has it is taken once;
  and it drops them for a chunk it sends in a `Return` or a `Lending`, the store sending
  them again where that is called off. Arrivals kept in memory are not part of a
  region's state: an absorbed region's come to the absorber by the store's resend,
  behind `Absorbed`.
- **Takings are checked by holder.** The store's check of a commit's takings
  (`lanes.rs:1063-1068`) passes over a number silently where the chunk is not in use;
  for an arrival that would leave the entity both `Whole` in the commit and filed. A
  taken arrival is checked against the holder (`Table::holder` is the committing region
  and the arrival is filed), and a commit that names one failing it is refused as a
  malformed stream.
- **A holder that cannot take it yet**: in a handshake, in `Preparing`, being moved, or
  restarting: the arrival stays filed and is sent again at the holder's hello, and with
  the holding at a merge or a split: every arrival whose `Table::holder` the merge or the
  split changed, pinned areas the survivor took over among them, goes on its new
  holder's stream (not with `Uses::moved`, which moves only what is in use,
  `lanes.rs:2648`). An arrival is never dropped: not at `Dropped` or `Unreadable`, where
  effects are (ADR-0025, section 5). After `Unreadable` it waits in the store, and is
  taken when the chunk is made again and loaded; that is a named loss path to measure, not
  a rule to rely on. A large arrival written forward with a segment is split over records
  as when it was first written.
- **Large.** An entity larger than a log record's room (`MAX_RECORD_LENGTH`: a chest
  minecart, a shulker box item) is split over records of its group as a chunk is.

**Fast entities.** An entity that moves more than two chunks in a tick (a pearl, an
entity launched by TNT, a fast minecart) can end in a chunk that is neither loaded nor
copied: its movement reads such a chunk as the game reads one with no chunk for
collisions (`getChunkForCollisions`, to read in N4), and it is sent there as any other.

**Vehicles and riders.** The root vehicle and all its passengers are one unit: they
belong to the root's chunk, and are `Whole` or `Gone` together when the root changes
chunk. A unit with no player in it crosses as one arrival. A unit with players crosses
as its vehicle and non-player passengers in one arrival, and each player by their own
hand-over through the edge, in the same commit. **While a player rides, their hand-over
is made when the root changes chunk**, not by the player's own position (a rider sits
offset from the root, on a camel or a happy ghast by more than a block), and goes to the
root's new chunk's region; the player's `PlayerTransfer` names the vehicle's key, its
chunk and their seat. A riding player's **holding chunk** is their root vehicle's, and
it is what every judgement of where a player belongs uses: the arrival (`apply_arrival`,
`region.rs:1161-1162`, `1220-1240`), the check each tick of whether to hand the player
on, the split's seeds and who goes (`reshape.rs:290`, `331`), the crowds the coordinator
sees (`region.rs:533`), and use and claims (`settle_chunks`, `region.rs:2254`); the
edge's view still follows the player's position (`fanout.rs:2690-2693`). A rider whose
vehicle goes into a chunk their region knows as `Unknown` stays in the region, **seated,
vehicle pending**, with the vehicle's new chunk as their holding chunk, until the region
learns who holds it: a `Claimed { granted }` (land nobody held, granted with the
vehicle's arrival) and the region takes its own vehicle back and seats the player; a
`Foreign` (another region holds it) and the player is handed on, naming the vehicle's
key and chunk. The 100 ticks on both sides cover the rest. The region the vehicle arrives in seats each player as they
arrive, in either order. A player who comes before their vehicle is **seated, vehicle
pending**: no dismount and no `SetPassengers` is sent, the latest `MoveVehicle` of their
client is kept and applied, through the usual check of a move, when the vehicle is
taken, so that a boat rowed across a line does not jump back; only seated players are
reported among a vehicle's passengers. A player whose vehicle has not come within 100
ticks is dismounted where they are, and is never seated later. The vehicle's region
drops a pending player passenger who is not seated within the same 100 ticks, or whose
hand-over is discarded (`Discard`) or passed over, so that a boat keeps no phantom seat. So a vehicle (a chest
boat with its contents) is never lost on the paths where a player's hand-over is passed
over, discarded or dropped with an outbox (ADR-0008), and two players in one boat cross
together. A leash, an owner, a target and a hanging entity's block are references by key
or position, read across a line through the copies; a leash whose holder is out of reach
of the copies breaks, as the game breaks one held too far (`Leashable`, read in N5).

**A player's** own crossing stays as it is (ADR-0006, ADR-0020).

### 6. What the edge shows: by key and `hops`

For entities other than players, the edge shows an entity by its key, from the report
with the highest `hops`. **Players keep today's rule** (ADR-0015, section 2.1; ADR-0020,
section 6: `highest`, `Shown::from`, `stands_for` and presence answers), with keys in
place of ids.

`hops` is raised by the region that **takes an entity in**, in the `Whole` commit that
takes it: at an arrival; at **every** load of a chunk from the store, a load after the
region's own restore among them (a lend's borrower, a claim after a return, a reload
after `Dropped` all come by entities so); and, for a split's part, for each entity of a
warm chunk when the part takes that chunk in. A merge raises nothing: the edge's
`stands_for` already takes the absorbed region's reports as the absorber's (ADR-0015). A
restore's chunks are loads and raise; nothing else does. Every raise writes the entity
`Whole` into the log: measured in N2 with the moving entities. A snapshot of a chunk for an
edge is made after the tick's raises, and held with its tick until that tick is durable,
as snapshots are now.

- a report (a spawn, a move, data, an event, a snapshot) with `hops` above what the edge
  shows takes over: the entity is the reporting region's from then; one with lower `hops`
  is passed over; one with equal `hops` counts only from the region the edge shows it
  from or the region it stands for;
- **a sent entity is reported "handed on"** with the `hops` it had before the crossing,
  never as removed: the edge keeps showing it until the receiver's report at the higher
  `hops` takes over. **The taking reports the entity whole** (a spawn's kind of report,
  with its `hops`): the edge introduces it where it does not show it (a snapshot of the
  chunk made before the taking removed it as handed on and not there), and otherwise
  continues it on the wire with no removal and no new spawn. A snapshot's removal keeps
  the key at the "handed on" `hops`, so that the higher report takes over. A sender restored after a crash that lost the sending loads the entity again
  with `hops` one higher (a restore's loads raise), and its moves take over;
- a removal (death, pick-up, despawn) carries the entity's `hops` and is passed over if
  lower than shown. The edge keeps a removed key with its `hops` until every region it
  hears from has reported 600 ticks, in that region's own ticks, after the first report
  of it the edge heard following the removal, so that a late snapshot or
  "handed on" from an old holder does not bring back what died or was picked up; keys
  are never reused, so nothing else needs it;
- a "handed on" never introduces an entity the edge does not show;
- an entity reported in a chunk the edge's replica (the chunks it watches,
  `fanout.rs:2710`) does not have (handed into
  land no viewer of the edge watches) is removed from that edge's viewers, through
  `refresh_entity`'s removal: `upsert_entity`'s early return for such a chunk
  (`fanout.rs:2710-2713`) must not skip it;
- a snapshot of a chunk removes the entities shown in that chunk as "handed on" that the
  snapshot does not have, from whichever region (a crash of the sender that lost an
  arrival the edge had been told of): a flicker, rare;
- with several edges, reports of two regions reach an edge in either order; `hops` orders
  them.

This answers ADR-0006's open question on versions for entities other than players.

### 7. Effects on entities across a line

What one entity does to another across a line goes as an **entity effect**, addressed to
the chunk in which the sender's copy shows the entity and to its key, carried and taken
as block effects are (ADR-0025, sections 4 and 5), on the game's tick while coupled. The
holder looks the key up among all its entities and passes the effect over if it is not
there, as the game's hit would miss:

- a hit, with its damage source (by key), amount and knockback;
- a push (`Entity.push`), with its vector;
- an explosion's damage and knockback, with its blocks as block effects (`SetIf`).

**Pick-up** happens only in the player's own region: a player picks up an item in their
box that their region has; an item across a line is picked up once it or the player has
crossed. In creative `Inventory.add` never fails (what does not fit is destroyed:
`Inventory.add`, `hasInfiniteMaterials`), so a pick-up needs no way back.

Where regions follow players they **merge** at once when entities work at their line, as
for circuits (ADR-0027, section 7): an entity within four blocks of the line whose holder
has sent an arrival across it in the last 600 ticks, or which targets or is targeted
across it. Pinned lines use arrivals and effects.

### 8. Spawning's counts

`NaturalSpawner` checks a world-wide cap (`SpawnState.canSpawnForCategoryGlobal`) and a
cap per player (`LocalMobCapCalculator`, the players within 128 blocks of a chunk), and
reaches eight chunks from a player; copies reach one. The store therefore publishes on
every region's stream, with each mark, the mobs by category of each chunk that changed,
and the world's totals and spawnable chunk count; a region reads them for the chunks
around its players and for the world-wide cap, a few ticks late. Built in N9 with
monsters; N7's animals spawn with their chunk.

## Consequences

- Entities survive crashes, merges, splits, lends, returns and moves of a region as
  blocks do, by the same records; nothing in those is decided per entity.
- A world made before N1 is refused (format 3) and made again.
- The log grows with every moving entity, and the copies' updates with it; measured in N2
  and N7, the plan's cheaper rule kept for if they cannot keep up.
- An entity crossing an uncoupled line stands still for the ticks its arrival takes, and is
  in neither region meanwhile: missed by pressure plates, hoppers, counts and pick-ups in
  that time.
- Mobs that target or flock across a line read copies a tick old, and may hesitate there.
- A durable write lost with its machine (the write-back cache) can leave an entity shown
  from a receiver whose taking was lost, at the higher `hops`, until the edge's viewers
  move away: rare, and only without the cache's safety.
- Named differences: a player whose vehicle arrives after them rides nothing until it
  does (section 5); spawning's counts a few ticks late (section 8); the three uses of HotSpot's `Math.sin`/`Math.cos` (the Q
  throw, elytra flight, TNT's priming) ported with `StrictMath` (the plan, 3.6).

## Steps

As the plan's section 4: N1 keys and ids (section 1); N2 entities in chunks, durable, the
item at rest, the whole inventory (sections 2 and 3); N3 crossing, vehicles and entity
effects, the edge by `hops` (sections 5 to 7); then the ported types, N4 to N10. N1 and N3
are not delegated; their tests are written from this record by someone who did not build
them.

## Review

**First review (2026-10-18), against the code at `98e8311`.** Not ready; worked in:

- D1, arrivals passed over by key, which takes an entity twice where it was taken, sent on
  and its holder restarted: arrivals are numbered and taken as a set, as effects are
  (section 5).
- D2, an arrival written into land nobody holds with no record that it was: a dropping
  record once the write is synced, and a claim's load waits for it. Superseded by the
  third review's change: no arrival is written into a stored chunk but at a fold.
- D3, `next_block` is worked out from the region files, not kept in the table: every block
  is written to its region's file before it is given (section 1).
- D4, `hops` frozen on screen after a sender's crash: "handed on" carries the `hops`
  before the crossing (section 6).
- D5, a late snapshot taking an entity back to its old holder: equal `hops` count only
  from the region shown or stood for, and `hops` is raised by whoever takes an entity in
  from outside its memory, a load among them (section 6).
- D6, a vehicle carried in a player's hand-over lost where the hand-over is: the vehicle
  crosses as an arrival in the same commit, and players are seated when it comes; two
  players in a boat cross together, and the named difference is gone (section 5).
- D7, a wire id reused before a client's late interact: kept until a ping after the
  removal is answered (section 1).
- G1, arrivals for land held but not used never delivered: the holder loads the chunk
  for them with an arrival ticket (section 5).
- G2, a part's block in the split's group is redundant: it comes with the part's hello
  (section 1).
- G3, G4: players keep today's rule; who raises `hops` and when (section 6).
- G5: the records amended are named (the status).
- G6: a snapshot removes entities handed on into its chunk that it does not have
  (section 6).
- G7, G8: the early return in `upsert_entity`; the log kind's name; a record deleted with
  its last entity; copies' bases of one sequence; a synced write per region at a store
  restart (sections 1, 3, 6). The vehicle of a player arriving into their own chunk not
  yet loaded is an arrival like any other, taken once the chunk loads (section 5).

**Second review (2026-10-18), of the record as revised.** Not ready: the first review's
fixes closed their holes, but arrivals were filed by holder and sent again by use, and
riding across a line showed seams. Worked in:

- 1: arrivals are filed and sent again by holder, not by use (section 5).
- 2: a chunk that stops being held (a return, `Withdrawn`, the store's start) sends its
  arrivals the way of land held by no region (section 5).
- 3: an arrival written into land nobody holds, then granted, then the store restarted
  before the dropping was durable: a sweep at the store's start. Superseded, as are 7
  and 8 below (a record ahead of the blocks, copies based again after a write), by the
  third review's change.
- 4: a rider's hand-over is made when the root changes chunk, to the root's region
  (section 5).
- 5: a rowed boat jumping back: the player is seated with the vehicle pending and their
  client's latest move kept (section 5).
- 6: after `Lent` the lender drops the arrivals it was sent; no arrival ticket loads a
  chunk lending or given back (section 5).
- 7: an arrival into a chunk never generated; generation keeps the record (section 5).
- 8: copies of land nobody holds are based again after a write (section 5).
- 9: fast entities into chunks neither loaded nor copied (section 5); the reason for a
  fresh block is a durable write lost with its machine (section 1).
- 10: every load raises `hops`, a restore's loads among them; a part's warm chunks when
  taken in (section 6).
- 11: removed keys kept with their `hops`; a "handed on" never introduces; the ghost of a
  lost taking named (section 6, Consequences).
- 12: `next_block` in a synced file of its own; `keys_low` named once (section 1).
- 13: the cost of raising at loads measured in N2 (section 6).
- 14: large arrivals written forward split; `DamageEvent`'s ids off by one; the edge's
  replica named (sections 1, 5, 6).

**Third review (2026-10-18), of the second revision.** Not ready: the store's write of an
arrival into land nobody holds, with its sweep at start, still lost or doubled entities
where a grant came between the filing and the write, or before the dropping was durable,
and a lent chunk given back is not nobody's land. Worked in by taking that path out:
**every arrival has a holder** (section 5). Land nobody holds is granted to the sender in
the arrival's group; a return of a chunk with arrivals filed is called off by them; a lent
chunk given back passes them to the holder the release leaves; a return or a lend called
off sends them again; only the fold at a change of division, with nothing running, writes
arrivals into stored chunks. Also: takings of arrivals checked by holder and refused
rather than passed over (its D5); riders judged by the vehicle's chunk (D6); phantom seats
dropped (G1); arrivals move with the holding at merges and splits, not the use (G3);
`Withdrawn` is no change of holder (G4); a restored sender's `hops` (G5); the edge's 600
ticks counted per region (Q1); blocks reserved sixteen at a time, and the start's guard
against a damaged `regions/blocks` (Q2); snapshots made after the tick's raises.

**Fourth review (2026-10-18), of the third revision.** Not ready, each finding with a
contained fix, worked in (section 5): the `Granted` record before the commit's, and
undone with a cut group (its A1); the sender told by `Claimed` in the arrival's group, by
nearness at a split (A2); a chunk held only for an arrival given back when its ticket
ends (A3); `Return` is never answered, so a return with arrivals filed is answered
`Claimed` and its arrivals sent again, for those filed before the `Return` and after
(B1, B2), and only where the release leaves no holder (B3); a worker keeps arrivals only
for chunks it holds or is about to hold, by number, and drops them at a `Return` or a
`Lending` (C1, D1), the lender keeping them until `PinnedBack` (C2); the fold at a change
of division drops what it wrote, durably, before the `Opened` records (E1); a riding
player's holding chunk is the vehicle's throughout the sim (R1, R2); the review's
superseded items marked, and the status line.

**Fifth review (2026-10-18), of the fourth revision.** Ready after small fixes, worked
in: the taking reports the entity whole and may introduce it (its G1, section 6); the
runner, not the sim, judges what a worker holds for its kept arrivals (G2); the arrival
ticket lasts until the arrival is taken, and an unreadable chunk holds none (G3); any
grant of a chunk with arrivals filed sends them (G4); a rider whose vehicle goes into
land their region does not know (Q1); `Claimed` as a reply in the group, the lender's
order behind `PinnedBack`, a commit checked before its grant, the fold's chunks (Q2).
The record is accepted with these.
