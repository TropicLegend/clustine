# Review of `docs/groundwork/p-plan.md` (the Nether and the End)

I read the plan, the brief and the survey, ADR-0020 and ADR-0028 (sections 5 and 6), the empty-region part of ADR-0016, and the code they rest on: `fanout.rs`, `messages.rs`, `table.rs`, the coordinator's `state.rs` and `follow.rs`, `region.rs` and `api.rs`. I built nothing and ran nothing. Every citation the plan gives that I checked is accurate (`fanout.rs:3291-3296`, `:5476`, `pool.rs:463-467`, `bot.rs:141-150`, `table.rs:532-558`).

The overall shape is sound. The real problems are in four places:
- **Section 3.3:** how a region gets made for a dimension.
- **Section 5.2:** the travel protocol's channels, and its rule that drops the answer when the traveller leaves the portal.
- **Section 4:** what other screens show of a traveller, and late messages from third regions.
- **Section 3.2:** the contracts are incomplete, so P2 would be reopened.

## 1. Defects

### D1. Section 3.4 misses the second place where the coordinator merges regions. An empty Nether region would be absorbed into the overworld's home region (sure)

The plan only makes the distance infinite (`policy.rs:50`, `:161`). Empty regions are absorbed by a separate path that ignores distance altogether. At `services/coordinator/src/state/follow.rs:1060-1072` the survivor is "the home region, then the regions with a lower id". Any Nether or End region that has had no players for `EMPTY_FOR` (30 s) would go into the overworld's home region. That region would then hold two dimensions, which breaks decision 0.1.

An item or mob thrown into a portal is enough to cause it: a region is made for the Nether, nobody is in it, and 30 s later the overworld absorbs it. `policy::places` also adds the home region's place at the overworld's entry chunk (`policy.rs:195-197`). So the dimension has to be part of the `Place` itself, not just of a distance.

**The plan should say:**
- A survivor must be of the same dimension.
- With no home in a dimension, the last empty region of that dimension stays (or see the per-dimension home in G1).
- The store refuses an `AbsorbCommit` across two dimensions, as a second line of defence.

### D2. Section 3.3's mechanism for making a region does not match how regions come into being (sure)

**"Placed as a split's half" is not how a half is placed.** A split's new half is not placed by the coordinator. The splitting worker opens it with the epoch the coordinator reserved (`messages.rs:615-621`). A region the store makes has no such worker. What actually exists is `Coordinator::listed`: "a living region of the list that the coordinator does not know is added, without an owner, and assigned" (`state.rs:1602-1604`). Two consequences:
- That path runs only once per list reading, every `LIST_EVERY`, which is one lease, 5 s.
- It runs "not while a split is reserved" (`state.rs:1699`).

So the first travel into a dimension waits up to about 5 s, plus the time to open the region and generate its chunks.

**"The coordinator told" has no channel.** No store-to-coordinator message exists; the coordinator only reads the list.

**"The claimant answered `Elsewhere`" names the wrong message.** `Elsewhere` is a `WorkerToEdge` message (`messages.rs:236`). A claim is answered with `StoreReply::Claimed { granted, foreign }` (`messages.rs:593-596`).

**"Once the coordinator has placed it" depends on something the store cannot see.** The store only learns of placement when the new owner says hello. That is a wait on a third process, which ADR-0020 rejects ("every wait between two regions is a place for an ordering mistake"). Nothing needs that wait: anything filed durably for the chunk reaches the holder at its hello (ADR-0028, lines 217-219). The store should answer at once with the new id, as advice, the way `Enter.holder` is advice.

**Id race with splits.** A made region uses up `next_region`. A `SplitCommit` already under way must name exactly the next id, or it is declined with `NotNext` (`messages.rs:467-471`). The split then has to be redone, since its state names the id. The plan should say so.

**3.3 and 5.2 disagree about when a region is made.** Section 3.3 makes one only for "a dimension that has no region". Step 5.2.2 makes one whenever no region holds the target chunk. The plan has to choose, and say who gets an unheld chunk when the dimension already has regions. Two travellers aimed at different unheld Nether chunks fall exactly into this gap.

### D3. The store still believes pinned overworld regions hold every chunk of the Nether and the End (likely)

`Table::holder` falls back to pinned areas by `ChunkPos` alone (`table.rs:363-369`), and pins are `ChunkArea` with no dimension. The end-to-end runs with `CLUSTINE_TEST_PINS=0,4` cover all land with pins. Unless `holder` and `pinned_holder` check that the chunk is in the overworld, every Nether chunk would be "held" by an overworld stripe. Travel would then go to that stripe.

Section 3.3 says "every chunk-keyed map by `LevelChunk`", but pins are not a map. The plan says pins are the overworld's only in the coordinator section (3.4); it should say it for the store's table as well.

### D4. `RegionConfig.spawn` would make a Nether region keep one chunk forever (sure)

`region.rs:145-147` says the chunk of `spawn` is "the home chunk, which a region that holds it never gives back". The store also leaves "the chunk players enter the world in" out of `Return` and `SplitPart` (`messages.rs:429-431`, `:521-523`). Without a dimension on that test, a Nether region holding Nether chunk (0, 0) would never give it back.

**Fix:** the spawn should be optional in `RegionConfig` and `Restored`, and in the overworld only. The store's home-chunk test should compare a `LevelChunk`.

### D5. Step 5.2.5 ("left the portal → answer dropped") stops creative players from travelling at all, and they are the players Clustine has (sure about the rule, likely about how often)

A creative player's transition time is 0 (G 2.1), so the portal fires on the first tick they are inside. Every other entity also fires on its first tick. A walking player is inside a one-block portal for roughly 6 to 7 ticks; a flying one for 3 or fewer.

The plan's round trip is at least three durable commits: the source's request, the destination's portal and answer, and the source's `Departed`. Each waits for a sync. On one machine with the memory cache that is about 3 to 4 ticks. Across workers, on 22 ms disks, or when a region must be made first, it is far more.

So a creative player who walks or flies through a portal, an item thrown through one, or a boat rowed through one regularly comes out the other side without travelling. In the game, once `handlePortal` fires the entity travels whatever it does next (G 2.1, step 3).

**The plan should say:** a traveller is committed at the moment the portal fires. The pending travel is part of the entity's durable state (`STATE_FORMAT`), and only removal cancels it: death, leaving the game, being picked up. The arrival is computed from the entry pose. Section 1.1 and question 2 should then describe the delay as "arrives a few ticks after, wherever they walked meanwhile".

### D6. Step 5.2.1's channel cannot carry what 5.2 needs (sure)

- **A remote click's outbox entry goes to an edge, and mobs have no edge.** A `Durable::Remote` goes "to the outbox of the edge the player belongs to" (`api.rs:1005-1010`), and the edge routes it. Mobs, items and boats have no edge, so "an outbox entry, as a remote click is" cannot carry their travel.
- **The plan mixes two channels.** Step 2 has the store answering. But an edge outbox entry never reaches the store.
- **The answer has no address.** Step 3's "answers the source" names no channel and no address. Between request and answer the source region can be merged, split, moved or taken over, and the traveller can have walked into another region.

**The plan should say how request and answer travel.** One way that already has all the needed properties is the arrivals and effects machinery (ADR-0028, section 5; ADR-0025):
- The request is filed in the source's commit for the target `LevelChunk`.
- The store makes or names the holder and puts the request on that holder's stream.
- The destination takes it once, in the same commit that makes the portal and files the answer.
- The answer is filed for the traveller's `LevelChunk`, so a merge or split carries it to the new holder, as filed arrivals go with the holding.

An even shorter design files the traveller itself as a "find or make a portal" arrival in the destination. The destination region then searches and places the traveller on taking it. That removes the return leg altogether.

Whatever the choice, "taken once" and "in the same commit as the portal's blocks" must be written down. Without them, a destination restored after a crash can make a second portal or answer twice.

### D7. The portal index lags by up to five minutes, so return trips make second portals (sure)

Decision 0.7 and section 3.3 build the index "with every saved chunk". Chunks are saved at checkpoints, `DEFAULT_CHECKPOINT_INTERVAL = 5 * 60 * 20` ticks (`services/worker/src/lib.rs:80`), and when given back.

A player who lights a portal, crosses, and comes back within five minutes is searched against an index that does not yet have their own overworld portal. A new portal is made beside it. That happens in ordinary play, within minutes.

**Fix:** keep the index from the block changes of every commit the store takes (`Commit.changes`). It is then durable with the log and current as soon as the tick is. The destination region must also merge in its own live points of interest. Otherwise two travellers handled one after the other (D2's race, or two requests in one tick) each make a portal.

### D8. Other screens keep a ghost of every traveller at the portal (sure about the mechanism)

**Non-players.** Step 5.2.4 files a non-player "exactly as an entity walking over a line is filed now". Such an entity is reported `HandedOn`: "an edge goes on showing it there until the region it went to reports it with more hops" (`api.rs:1722-1731`; ADR-0028, lines 395-400). An edge with no viewer in the Nether never gets a report from the Nether region. Its overworld viewers see the item or mob frozen at the portal until they walk away.

**Players.** The old region does not report a player it lets go as removed, "because it lived on" (`fanout.rs:3337`). Every other player on another edge, near the portal, keeps a still copy of the traveller.

In the game, `removeAfterChangingDimensions` and `removePlayerImmediately` send a removal to the old level's watchers (G 2.7, G 3.1 step 5).

**The plan should say:**
- A crossing to another dimension is reported to the old dimension as a removal carrying the entity's `hops`, so a late report cannot bring it back.
- The edge's `Shown` (`fanout.rs:981-993`) holds the entity's dimension, so visibility is worked out per dimension. The plan mentions only the replica and the players' views.

### D9. Late messages from third regions reach the traveller's new screen (sure)

`Durable::RemoteDone` from another overworld region is shown when `has_stay(player, entity)` holds (`fanout.rs:2188-2205`). The entity id survives the change of dimension, so a click finished just after the change sends `shown` blocks at overworld coordinates into the client's Nether. The same holds for a `Remote` with `to: None`: the edge routes it by "the region that serves the edge the chunk" and must look that up in the dimension of the region that sent it, not the player's.

The plan's "late deltas and snapshots of the old dimension's regions are not shown" covers only the old region's own link.

**Fix:** remote actions and their answers carry the dimension (decision 0.2 already says cross-region messages carry `LevelChunk`), and the edge drops positional content whose dimension is not the view's.

### D10. The edge's change of dimension needs the existing hand-over order, which section 4 leaves out (likely)

`enter_by_arrival`'s comment (`fanout.rs:4698-4704`) records why the edge must do its own notes and send `PlayerArrive` before anything goes to the client. A client whose queue overflows is removed, and its leave would otherwise overtake the arrival. Section 4 lists only the client's packets.

**It should state the whole order:**
1. Unwant the old view by `LevelChunk`.
2. Switch the view's dimension.
3. Clear `sent`, `pending`, `replace` and `visible` (the client dropped its level and its entities), and the riding state.
4. Want the new view of `to`.
5. Send `PlayerArrive`.
6. Then send G 3.1's packets.
7. Then send chunks.

`told_clocks` must be reset, because the level information sends `SetTime` whole.

**What should trigger the change.** The plan triggers it "for a departure that carries a `Travel`". But a `NotMine` for an arrival is turned back into a `Departed` carrying the same transfer (`fanout.rs:11437`), and a bounced travel would then send a second `Respawn`. The trigger should be "the transfer's dimension differs from the view's", or the `Travel` must be consumed after its first use.

### D11. Dropping every kept input loses inputs that have nothing to do with position (likely)

Section 4 drops all kept inputs. Some of them are not positional: `SelectSlot`, `SetInventorySlot` (a creative inventory edit), `SwapHands`, `SetFlying`. The change then sends the inventory from the transfer, which is the old region's, so a creative player's last inventory edit silently disappears.

**The plan should say:**
- which input kinds are dropped: the moves and the actions aimed at blocks or entities;
- how the inventory sent with the change agrees with the inputs that are replayed.

Separately, actions aimed at blocks that the client sent before it read `Respawn` reach the edge after it. They should be held back until the teleport is accepted, as moves are. (The region's reach check probably catches most of them; I am less sure here.)

### D12. ADR-0028's grant of unheld land to the sender breaks "one dimension per region" (sure)

An arrival into land no region holds is granted to the sending region (ADR-0028, lines 239-247). A cross-dimension arrival, or a travel whose arrival chunk was given back between the answer and the arrival, would put a Nether chunk into an overworld region. The plan must change that rule for `LevelChunk`s of another dimension, sending them to D2's maker or G1's per-dimension home.

## 2. Gaps

**G1. A player who logs in with a place in a dimension no region holds (sure).** In ADR-0020's step 7, case 2 (`holder` is `None`), the home region places the player at their place and claims the chunk (ADR-0020, line 295). The home region is the overworld's, so a Nether place would be put into an overworld region. This comes up easily:
- a player leaves in the Nether and their region is given back;
- an edge restarts in the middle of a change: the source's `Has` note already gives the Nether place (ADR-0020, section 3, "let a player go").

The survey asked this (survey section 7, question 8); the plan says nothing.

The simplest answer covers G1, D1, D2 and D12 together: **one home region per dimension.** Each would be made with the world, never absorbed, holding no pinned land and nothing until somebody needs it. It would place returning players and take unheld chunks of its own dimension, exactly as the overworld's home does today. That removes making regions on demand, the store-to-coordinator signal and the first travel's wait of about 5 s. I recommend the plan weigh this against section 3.3 as it stands.

**G2. Contracts that later steps need, missing from section 3.2 (sure).** As written, P2 would be reopened by P3, P8, P9 and P12. Missing:
- the travel request and answer, as messages or filed records (P8's portal clock "asks", but P8 runs before P9 has designed what it asks with);
- the travel pending in a region's state, and the `PortalProcessor`'s state (`STATE_FORMAT`);
- the portal index's log records and its query (`FORMAT_VERSION`);
- the made region's record in the table, or the per-dimension homes;
- the dimension in `Shown`, `HandedOn`, `EntityGone` and `RemoteAction`/`RemoteDone`;
- `PlayerTransfer.riding`'s vehicle chunk as a `LevelChunk`;
- `RegionConfig` and `Restored`'s spawn made optional (D4) and the light's sky;
- the dragon fight's world record (P12, "the lead's");
- consecutive wire ids for the dragon's eight parts. G 6.4 says clients number them dragon + 1 to + 8, and `WireIds` is the edge's, which P12's subagents may not touch.

Either define these in P2, or say plainly which later step raises which version.

**G3. The region's light without a sky.** `region/light.rs:100` always builds `Lighting::new(true)` (survey section 3). The plan fixes the edge and the scheduler only. With a sky under the Nether's roof, the spawn-light rules of section 6 read the wrong light.

**G4. Mixing the dimension into random ticks.** `Restored.world` seeds random ticks by chunk, section and tick (survey section 1.2). Nether and overworld regions at the same coordinates would draw the same streams. This is small; it should be named or fixed in P2.

**G5. Keeping chunks loaded where the game has a ticket.**
- **The dragon's arena.** `isArenaLoaded` needs chunks x −8 to 8 at z = +8, which the game keeps loaded with a radius-9 `DRAGON` ticket (G 6.1, tick steps 3 and 4). A region following a player on the platform at (100, 49, 0) with view distance 8 loads chunks x −2 to 14. Without an equivalent ticket the fight never starts.
- **The portal ticket.** Step 5.2.3's "places a portal ticket" has no Clustine counterpart named. The nearest is ADR-0028's arrival ticket.

**G6. Making a portal across regions.** "Borrowing what it does not hold as a remote click does" is not an existing mechanism. A remote click goes to the holder; lending (ADR-0027) is only for pinned areas. The 33 × 33 spiral and the portal's blocks can fall in other regions' chunks. The plan should say:
- reading is through border copies (ADR-0025, section 3);
- writing is through effects, or by claiming unheld chunks;
- the arrival goes to whichever region holds the arrival chunk.

**G7. Riders and vehicles.** The vehicle travels and carries the player (G 2.7). The plan should say:
- one request covers the root and its unit;
- the player's `Departed` and the vehicle's arrival go to the same destination;
- the client, which dropped the vehicle with `Respawn`, is told `SetPassengers` once the vehicle is introduced in the new dimension.

**G8. The End's exits read a respawn block in another region.** Leaving the End by its portal and by the credits uses `findRespawnPositionAndUseSpawnBlock` (G 2.4, G 2.5): is the bed or anchor still there, and does the anchor have charge? That is a read across regions, and maybe across dimensions, before the change, with `NO_RESPAWN_BLOCK_AVAILABLE` if the block is gone. Section 5.3 also does not say:
- where the player is during the credits, which can last minutes (G 2.5: the game removes them from the level);
- what happens to their inputs meanwhile;
- what a disconnect or an edge restart during the credits does.

**G9. The End's void.** Clustine has "no death" (ADR-0020, section 4). In the game, `fell_out_of_world` bypasses creative invulnerability. A player knocked off the End's island by the dragon falls forever. That happens within minutes in the End, and section 1.1 does not name it.

**G10. Smaller items:**
- the brief's section 9 classes "to dump before planning" (`EndPlatformFeature`, `EndPodiumFeature`, `LavaFluid`, `BlockUtil.getLargestRectangleAround`, `ServerLevel.findFreePosition`, `CommonPlayerSpawnInfo`; the `Respawn` wire form itself is only inferred) are in no step;
- the fallbacks at −64 and 319 (`falling.rs:605`, `items.rs:152`, `boats.rs:646`) are in no step;
- the change's position packet should use the game's relative flags (`DELTA ∪ ROTATION`, a yaw delta of 0 or 90) rather than an absolute pose that is already a few ticks old;
- a pearl's owner carried into another dimension is a change not made by a portal;
- re-dividing a world (switching `--pin`) folds the Nether's regions as well;
- no ADR is planned, though this changes what a region is.

## 3. The division of work

- **"Their crates do not overlap" is not so** (sure).
  - P8 (lighting a portal from fire), P11 (infiniburn and soul fire) and P12 (mob families) all edit `clustine-sim`. P8 and P11 share `region/random_ticks/fire.rs`. P11 and P12 share `spawning/rules.rs`, and P12's families all edit the dispatch in `mobs/types.rs`. P11 and P12 run "from P2 on, beside the rest", alongside P8.
  - P3 is "subagent and lead" in the same crate at the same time.
  - P3 needs P5's `TerrainGenerator::new(seed, dimension)` and the generator's identity in `services/worldgen`.
  - Fix the generator's constructor and identity in P2, and set worktree boundaries by file, not by crate.
- **Hidden dependencies.**
  - P6's End city shulkers and P5's End spikes (each spike gets an End crystal entity, G 6.3) need P12's shulker and crystal.
  - Bastion piglins and the Nether's generation-time mobs need P12's families.
  - Raising `ENTITIES_GENERATION` belongs after those families, not in P6.
- **Ordering-critical work is delegated** (likely). P8's portal clock "asks for the arrival", and whether a fired traveller is committed (D5) is an ordering decision. The answer must be fixed by the lead before P8, not left to P9. P4's change to absorption (D1) is ordering-critical and should be briefed precisely, or done by the lead with P3's region making.

## 4. The questions for the owner

The eight are the right kind. Changes I suggest:
- **Question 2** should be reframed around D5. The question is not "same tick or a few ticks", but "committed at the moment it fires, arriving a few ticks later wherever the traveller walked meanwhile", plus the first travel's wait if regions are made on demand.
- **Add:** a home region per dimension against regions made on demand (G1, D1, D2).
- **Add:** joining with a place in a dimension no region holds. The survey's question 8 was dropped.
- **Add:** the End's void and death, which bears on question 7 (G9).
- **Add:** the terrain plan's D3, a one-dimension world. Say it is dropped, rather than drop it silently (survey's question 6).
- **Question 3** (refusing old worlds) is fine. It should say plainly that the owner's own play world will be lost again.

No proposal is one the plan should not make.

## 5. What is good and should stay

- A region holds one dimension. `LevelChunk` is used only where several dimensions are seen, and region-local maps stay keyed by `ChunkPos`.
- Travel is resolved before departure, as a hand-over, and the delay is named and measured.
- The edge's change follows G 3.1's order. `ForgetLevelChunk`s are not reproduced, and there is no `PlayerLoaded` wait.
- The portal search reads a store-side index rather than other regions' blocks (once D7 is fixed).
- The edge, travel and region making are not delegated, and kill tests at every step are required.
- `has_sky` is mended.
- 26.3's attributes are read rather than the old fields.
- The dragon's first fight is kept in the phase.
- The rules apply to Clustine's creative players as the game has them.

### Critical files for implementation
- `/home/claude/clustine/services/coordinator/src/state/follow.rs` (absorption survivors, `decide_and_begin`)
- `/home/claude/clustine/services/worldstore/src/table.rs` (`holder`, pins, region making), with `lanes.rs`
- `/home/claude/clustine/services/edge/src/fanout.rs` (`hand_over`, `enter_by_arrival`, `Shown`, `RemoteDone`)
- `/home/claude/clustine/crates/clustine-rpc/src/messages.rs` and `/home/claude/clustine/crates/clustine-sim/src/api.rs` (P2's contracts)
- `/home/claude/clustine/crates/clustine-sim/src/region.rs` (`RegionConfig.spawn`, arrivals)


---------------------------------------------------------------------------------------

# Second reading of `docs/groundwork/p-plan.md` (revised)

I read the revised plan whole and checked the new design against the code. As before, nothing was built, run or edited.

The revision fixes most of what the first review found. The per-dimension home is the right call, and committing the traveller when the portal fires is right. The new defects are mostly in how a **player** travels. The plan now sends a player by two paths at once. It splits the edge's change of dimension into two halves without saying what blocks the client's moves in between, or what word starts the second half. And the credits leave a stay with nobody to act for it.

## 1. Is each earlier point really answered?

| Item | Status |
|---|---|
| D1 cross-dimension absorption | Answered (3.4: both paths, plus the store's refusal) |
| D2 regions made on demand | Answered by the homes. But see N2: the `Departed`'s `to` cannot be "the region the store names" |
| D3 pins | Answered (3.3) |
| D4 spawn chunk | Answered (3.2) |
| D5 creative players walk out | Answered in principle (0.6) |
| D6 channel and addressing | Answered for entities other than players. For players there are now two channels (N1) |
| D7 index from saves | Answered. One caveat: the index records must survive the log's compaction at checkpoints, or be rebuilt. "In the store's log" alone does not say which |
| D8 ghosts | **Only half answered.** 5.1 step 4 reports a removal for "any other entity". A travelling **player** is still left as a still copy on other edges (`fanout.rs:3337`), because players are shown by the player rule, not by `EntityGone` (ADR-0028, section 6). Sure |
| D9 late content | Answered, with a trap (N6) |
| D10 order and trigger | Answered for the first half. The halves bring new problems (N3, N4) |
| D11 inputs | Answered |
| D12 grant to the sender | Answered (3.3) |
| G1 login with a Nether place | Answered in 0.2. ADR-0020's step 7 should say the case explicitly: `holder` is `None` and the place is in another dimension, so case 3 applies with X = that dimension's home. Case 2 is not right there |
| G2 contracts | Mostly. Missing: the word that places a travelling player (N3), the credits' state and `seenCredits` (N8), a home's identity in the coordinator's configuration, and what a Traveller still waits for at the destination (N5) |
| G3, G4, G9, G10 | Answered |
| G5 tickets | Answered. The arena claim should begin when the boss bar first has a player (G 6.1, step 3), not "while the fight runs": the fight cannot run until the arena is loaded |
| G6 making a portal across regions | Answered, with caveats (N5) |
| G7 riders | Partly (N7) |
| G8 the End's exits | The exits are answered. The credits are not (N8) |

## 2. Defects in the new design

### N1. A travelling player goes by two paths at once (sure)

Four places in the plan describe the player's journey:
- 3.2: a `Traveller` holds "the entity, **or the player's transfer**".
- 5.1 step 2: the Traveller is filed for the target chunk.
- 5.1 step 3: the player also gets a `Departed`.
- 4.2 step 6: `PlayerArrive` carries the transfer "which carries the `Traveller`".

So the same stay reaches the destination dimension twice:
- once as a filed arrival, taken by the holder of the target chunk;
- once as `PlayerArrive` through the edge, sent to the `Departed`'s `to`.

These can be two different regions (see N2). A region that gets a second arrival of a stay it already has passes it over. A passed-over arrival sends `Ended` to the edge it came through (`api.rs:1005-1010`), and the edge then puts the player out.

**The plan should pick one path for a player.** The edge path is the one ADR-0020 orders: `PlayerArrive` to the destination's home, which finds or makes the portal if it holds the target chunk, and otherwise passes the player on by `NotMine`. The filed path should stay for entities other than players. Riders need care either way (N7).

### N2. The source cannot name "the region the store names for that chunk" (sure)

5.1 step 3 makes the `Departed` "to the region the store names for that chunk, or to the destination's home". But the region builds `Durable::Departed { to }` in the tick the portal fires. That is before any commit, and a region learns nothing of another dimension's holders in-tick: its claims and answers are of its own dimension.

So `to` is always the destination's home. Whenever another Nether region holds the target, which is common once Nether regions follow players, the travel takes an extra hand-over by `NotMine`. That bounce sits inside the loading-screen wait, so it belongs in section 11's measurement. Say so.

### N3. The second half of the change has no word to start it, and the client's moves between the halves are not blocked (sure about the code; likely about what the client does)

**No word starts the second half.** 5.2 step 5 says the edge learns the placed pose from "the region's `Spawned`, or its `Presence`". Neither fits:
- The edge takes `Spawned` only for a view with **no entity** and the join's attempt (ADR-0020, section 4.1). A travelling stay has an entity.
- `Presence` is only an answer to a hello.

P2 needs a new word, for example `PlayerEvent::Placed { pose, flying, … }`, from the region that takes the player, published after its commit.

**Moves are not blocked between the halves.** The edge ignores movement only while `awaiting_teleport` holds the id of a teleport it has sent (`fanout.rs:76-82`). The plan sends the new teleport id in the second half. Between the `Respawn` and that position packet, nothing is awaited. Whatever moves the client sends from its fresh level then go to the destination as moves of the travelled stay, at a position the destination never gave. Set a sentinel in `awaiting_teleport` when the `Respawn` is sent.

**The plan contradicts the brief about the loading screen.** 0.6 and 1.1 say "the loading screen, which the change's `Respawn` has opened". G 3.1, item 13, says `LEVEL_CHUNKS_LOAD_START`, which the plan sends in the second half, is what starts the client's loading screen. If the brief is right, the client spends the gap in an empty level, can look about and move, and sees no screen. The client's handling of `Respawn` must be read (G 9 lists it as not yet dumped), or checked with a real client, before 4.2 is built.

**There is no patience for a destination that never places.** If the placing never comes (the arrival is passed over, or the destination is stuck), the client waits for ever. Only joins have a patience (`joining_since`). Give a travelling view a patience of its own.

### N4. A player who leaves, or whose edge restarts, between the halves can come back inside rock (likely)

4.2 says the stay's `Has` note "names the new place" from the source's commit. The source does not know that place. It knows only the target point: scaled x and z, unscaled y. The pose comes from the portal search at the destination.

A player who leaves before the placing returns at the target point, which in the Nether is often inside netherrack or lava. **Options:**
- keep the source's place in the note until the destination places the player;
- or keep the Traveller with the player's record, so that a rejoin runs the search again.

### N5. "Taken once, in one commit with the portal" needs a waiting step that the plan does not describe (sure)

Before the destination can take a Traveller it needs:
- the store's index answer (a round trip);
- loaded spiral chunks, which may still be generating;
- border copies;
- grants for the unheld chunks it claims (`Claimed` arrives after a commit, and may come back foreign).

ADR-0028's rule, "ready once its chunk is loaded", does not cover any of these. The plan should state:
- **The waiting state.** It is not durable. A take-over during it starts again at the hello.
- **Two travellers in one tick.** They are taken in order, and the second sees the blocks and points of interest the first set within that tick.
- **No checks in far chunks.** "Keeps those whose block is still a portal" can only check the destination's own chunks and copies. At radius 128, candidates lie in other regions' chunks; there the index (kept from commits) has to be trusted.
- **A half-built portal.** Writing another region's blocks uses `SetIf { expected }` (`effects.rs`, the variant `SetIf`). A block changed meanwhile is skipped, leaving nether-portal blocks without a full frame, which then collapse. That is rare; name it.
- **A residual race.** Two travellers handled by two regions within 16 blocks of each other's new portal, both uncommitted, make two portals. Also rare; name it.

### N6. Dropping another dimension's remote answer stalls the client's block acknowledgements (likely)

4.1 drops "a remote action's answer". If the whole `RemoteDone` is dropped, `arrived` is never called. The sequence then stays in `under_way`, and `acknowledge` caps every later acknowledgement below it (`fanout.rs:3185-3209`). The client is never told that later block actions are done.

**Fix:** drop only the `shown` blocks, and still call `arrived`.

### N7. Riders: neither channel says who decides the player's pose (likely)

The vehicle's `Traveller` is filed for the target chunk. The player's `Departed` goes to the home (N2). They meet only if the home holds the target chunk. The player's pose is the vehicle's pose plus the seat offset, and that is known only once the vehicle's Traveller has been placed.

ADR-0028's "seated, vehicle pending" rule lets a player wait up to 100 ticks, then dismounts them "where they are". A travelling player has no "where they are". The plan should say:
- the vehicle's placement decides the player's pose;
- the second half waits for it;
- what happens if the vehicle never comes.

### N8. The credits leave a stay that no region holds, and nothing can act for it (sure)

5.3 removes the player from the End and notes the stay as "in the credits". Then `PERFORM_RESPAWN` arrives at the edge, and the edge cannot file a Traveller: the edge does not talk to the store, and only a region's commit files anything.

Other consequences:
- The edge's `view.region` names a region that no longer has the stay. At that link's next hello the answer is `Absent`, and the edge puts the player out with "lost track" (`fanout.rs:1238-1256`, `1666-1683`).
- `seenCredits` and the "in the credits" state are not among P2's contracts.

**Simplest fix:** keep the player in the End region, hidden and frozen, with a credits flag in the region's state. The End region files the Traveller when it gets `PERFORM_RESPAWN`, and presence keeps working.

### N9. Two smaller points about the homes (likely)

- **Splitting a Nether or End home.** When the home region splits, the group holding "the chunk players enter in" stays (`policy.rs:306-311`). A Nether or End home has no such chunk, so the plan must say which group stays in it.
- **Remaking the table.** When a world's division changes, the table is made anew (`Table::made_from`). That must make all three homes. The load-time check `holder(home_chunk) == home_region` (`table.rs:282`) applies to the overworld's home only.

## 3. Division of work and questions

**Division of work:** P8, a subagent's step, owns the portal clock. But "what firing does" is the ordering-critical part: removal, `Departed`, filing, riders and the removal for travel, all in one commit. Fixing its contract in P2 is not enough; the lead should write that tick (P9) and leave P8 the clock and the blocks.

**Questions:** they are right. Question 2 should mention the bounce through the home (N2) and the gap between the halves (N3).

## 4. Still missing, briefly

- The removal for travel for a **player** on other edges (D8).
- The placing word (N3).
- A travelling view's patience (N3).
- Whether the portal index survives the log's compaction (D7).
