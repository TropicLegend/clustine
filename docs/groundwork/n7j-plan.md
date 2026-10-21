# N7j: brains, and the animals that have them, planned

Draft of 2026-10-21, for the owner and an independent review. Step N7j of the second
half of N7 (`n7-plan.md`, section 11.5: "N7j brains, with the brain's encoding from its
own measurement"). The game's rules are the porting brief's section 7 (`n7-brief.md`,
cited "B 7.1.5" for its section 7.1.5, "B 7.0.3.9" for finding 9 of section 7.0.3) with
its sections 9.7, 10.7, 11.7 and 12.7; the bytecode is in `reference/n7-javap/`, cited
by the simple class name as the brief cites it ("Brain.txt 838–999" is
`net.minecraft.world.entity.ai.Brain.txt`). The decisions in force are `n7-plan.md`
section 11 (cited "plan 11.2"); this note decides what they leave open for brains and
says how Clustine builds them on what the first half and N7h and N7i built
(`crates/clustine-sim/src/mobs/`).

A brain is the game's second kind of mind (B 7.1): memories with expiries, sensors that
fill some of them every few ticks, activities that say which behaviours may start, and
behaviours of three kinds that run for a while. Fourteen types have one (B 7: the
axolotl, frog, tadpole, goat, camel, camel husk, armadillo, sniffer, allay, happy
ghast, nautilus, zombie nautilus and copper golem; the villager's is N7k's and N10's on
the same machinery, B 7.14).

## 1. What a player sees when N7j is done

Every one of these comes from its spawn egg, with the body, attributes, sounds, synced
data, loot and despawning the first half built for every mob (plan 11.2), and the
brain below. Nothing here is made with a chunk yet (N7l), nor spawns naturally (N9).

- **Axolotl** (B 7.4). One adult of a common colour (lucy, wild, gold, cyan; blue only
  from breeding, one in 1200). It swims, walks slowly on land and dries out after 6000
  ticks out of water. Hit in water it may play dead: on its side for ten seconds, its
  head still, not targetable. In water it hunts the fish, squid and tadpoles of N7i
  and this step within 8 blocks, then rests from hunting for two minutes. Tempted and
  bred by a bucket of tropical fish (a creative player keeps the bucket and is given
  a water bucket if they have none); babies follow adults. A water bucket takes it
  into a bucket of axolotl, which pours a fresh one (named in 1.2).
- **Frog** (B 7.5). Of its biome's variant (temperate, warm, cold). It croaks, makes
  long jumps of up to 4 blocks across and 2 up, liking lily pads and big dripleaf, swims,
  is tempted by slime balls and breeds with them; the bred frog goes to water beside
  land and lays frogspawn, which hatches 2 to 5 tadpoles after 3600 to 11999 ticks.
- **Tadpole** (B 7.5.8). Swims, flees players within 8 blocks, panics when hit, flops
  on land and drowns there; tempted by slime balls; fed one, it grows by a tenth of what
  it has left; after 24000 ticks of its own it is a frog of the biome it is in. A golden
  dandelion locks its age; a water bucket takes it.
- **Goat** (B 7.6). Wanders, long-jumps up to 5 blocks (with its leap and landing
  sounds and pose), rams other mobs and armour stands 4 to 7 blocks away (lowering its
  head first), never a creative player (B 7.0.3.15); a ram into a log, stone, packed ice
  or certain ores knocks a horn off. One in fifty is a screaming goat, one adult in ten
  has one horn. Milked with a bucket every click, tempted and bred by wheat.
- **Camel** (B 7.7). Wanders slowly, sits down for a while and stands up again (40 and
  52 ticks of animation), panics when hit (standing at once), eats cactus to breed and
  grows up, grazes like a horse. Saddled, it carries two players, the first steering;
  a jump is a dash, with its cooldown sound; sneaking and clicking opens its saddle
  slot. **The camel husk**: the same body as a monster, which never breeds, eats rabbit
  feet, despawns as a monster does, and is saddled and ridden alike.
- **Armadillo** (B 7.8). Wanders; a player who sprints or rides within 7 blocks (2
  up or down), or anything that hurts it, rolls it up: it rolls, lands scared, peeks out
  now and then, unrolls once it is quiet for four seconds. Rolled up it takes half of
  a hit less one. Brushing it gives a scute; it sheds one every 6000 to 11999 ticks.
  Spider eyes to breed, but not rolled up.
- **Sniffer** (B 7.9). Scents and sniffs; then walks to a spot of dirt, grass, moss or
  mud and digs for 160 to 180 ticks (its box lower), dropping a torchflower seed or a
  pitcher pod 120 ticks in, rises, is happy, and does not dig again for 9600 ticks.
  Bred with torchflower seeds it lays a sniffer egg item; placed, the egg cracks twice
  and hatches a snifflet after about 24000 ticks (12000 on moss), which grows for 48000.
- **Allay** (B 7.10). Flies. Given an item, it collects items of that kind within 32
  blocks and brings them to the player who gave it, or to a note block it heard played
  within 16 blocks in the last 30 seconds, throwing them one at a time; that player
  cannot hurt it. An empty hand takes its item back. It dances by a playing jukebox and,
  dancing, duplicates when given an amethyst shard (a creative player keeps the shard),
  each copy then waiting 6000 ticks. It heals and never despawns.
- **Happy ghast** (B 7.11). A ghastling (from its egg as a baby, or from a dried ghast
  block left under water, which wakes after four random ticks and 5000 ticks for each
  stage) follows players and adults, is tempted and grown by snowballs, and grows up.
  An adult floats about its home by impulses, tempted by snowballs and harnesses; with
  a harness it carries four players, steered by the first; a player standing on it
  holds it still, solid to stand on, its yaw snapped to a quarter turn.
- **Nautilus** (B 7.12). Swims, dries out on land, breeds with fish when tame, is tamed
  by a pufferfish (one in three, hearts or smoke), saddled and armoured, ridden under
  water by one player who steers where they look and dashes; a wild adult in water
  charges a pufferfish now and then. **Zombie nautilus**: never a baby and never bred,
  never panics, of a temperate or warm (coral) variant by its biome; tamed and ridden
  alike.
- **Copper golem** (B 7.13). Built from a carved pumpkin or jack o'lantern on a copper
  block (the block becomes a copper chest under it), or from its egg. It walks to the
  nearest copper chest within 32 blocks that has items, stands at it with the lid open
  for three seconds, takes up to 16 of the first stack, takes them to the nearest chest
  that is empty or has that item, and puts them in; it opens wooden and copper doors on
  its way and closes them behind it, and waits while a player has its chest open. It
  weathers a stage every 504000 to 552000 game ticks and, oxidised, soon turns into a
  statue; honeycomb waxes it, an axe scrapes it back a stage.

Across a pinned line every one of them is tempted by a player on the other side,
follows an adult, breeds and is followed across it as the farm animals are (plan 11.2);
a jump, a charge, a dash or a flight carries it over in mid-motion with its brain
whole, and a merge, a split or a crash's restore never stops a panic, a dig, a long
jump or a sort half-way.

### 1.2 Left out, and named

- **The frog's tongue** has no prey until slimes and magma cubes exist; see question
  7.3 for what N7j builds of it. **Froglights** come with magma cubes (N9).
- **Mob effects** (none until N9, plan 11.6): playing dead gives the axolotl no
  Regeneration, a hunt's end gives the player none and takes no Mining Fatigue away, a
  nautilus's rider has no Breath of the Nautilus. The memories and timings are the
  game's; only the effect is missing.
- **Item components** (the owner's question 4 of plan 11.7): a bucket of axolotl or
  tadpole carries nothing and pours a fresh one; a goat's horn drops as a plain goat
  horn without its instrument; a copper golem's statue keeps no name.
- **N9's riders and N10's gifts**: the camel husk's husk and parched, the zombie
  nautilus's drowned, and the iron golem's poppy for a copper golem's antenna (so shears
  take nothing until N7k's iron golem gives one). The zombie nautilus does not burn in
  daylight until N9 (plan 11.6).
- **Weather is clear** (plan 11.6): a happy ghast heals fast only in clouds, never in
  rain.
- **No statistics or advancements** (the allay's, the sniffer's, building a golem).
- The rest of this step's named differences are in sections 2 and 3 where they arise,
  and section 7 lists the ones the owner may want otherwise.

## 2. The brain's machinery

### 2.1 Where it sits

A new module `crates/clustine-sim/src/mobs/brain.rs` with `brain/` beside `ai/`, as
`ai.rs` foresaw ("The brain (N7j) comes beside"): `brain/memory.rs` (the memory types and
their slots), `brain/sensors.rs`, `brain/activity.rs` (activities, their order, the
provider), `brain/behaviour.rs` (the three kinds, gates, shuffled lists, the declarative
helpers), `brain/behaviours/*.rs` (the shared behaviours of B 7.3, a file each or a few
together), `brain/encoding.rs` (the record's form, section 2.9). A type's own
behaviours (B 7.3.24) live in its type's module.

The brain is the mind's, beside the goal selectors (plan 11.3, item 3): its parts are
mind entries, which copies leave out, so nothing of it reaches the copies' stream. What
it moves (paths, the controls' wanted states, the head) is the body's as it is for goals,
and copies carry that.

### 2.2 Memories

1. **A memory type is a Clustine id, fixed for good**, with its value's Rust type
   (`brain/memory.rs`, one table: `MemoryKind { id, name, value, sensed, saved_by_game }`).
   The ids are not the game's registry order, which may change between versions; the
   name is the game's (`memory_module_type` registry), checked by a test against the
   extraction's registry list. N7j defines the memories these fourteen types and the
   shared behaviours use (B 7.1.2, 7.4 to 7.13) and the villager's core and idle ones
   that 7.14.2 names; N10 adds more under new ids.
2. **A memory is a slot**: a value or none, and an expiry. The game keeps a time to
   live that the forget pass lowers by one every brain tick (MemorySlot.txt 21–41);
   Clustine keeps the **moment it expires**, a `lived` (section 2.6), so that a memory at
   rest is written once, when set. `getTimeUntilExpiry` is `expires − now`, read after
   the forget pass as the game's is (the armadillo's 75 and 30, B 7.8.3; the axolotl
   bucket's `HuntingCooldown`). The rule of B 7.1.2 holds exactly: a memory set with
   expiry `n` in tick T is present for the behaviours of T to T+n and gone in the forget
   pass of T+n+1; expiry 0 lives through T+1. `setMemory` without an expiry replaces any
   expiry the slot had; an empty collection is none (Brain.txt 380–409).
3. **An integer cooldown that `CountDownCooldownTicks` lowers** (`temptation_cooldown_
   ticks`, `long_jump_cooling_down`, `ram_cooldown_ticks`, `gaze_cooldown_ticks`,
   `charge_cooldown_ticks`, `attack_target_cooldown`, `liked_noteblock_cooldown_ticks`,
   `item_pickup_cooldown_ticks`, `transport_items_cooldown_ticks`) and `play_dead_ticks`,
   which `ValidatePlayDead` lowers, are kept as **the value at a moment and the moment
   its lowering began**, the lowering behaviour's own start (B 7.3.12: started in the
   tick after it was set, unless set earlier in the same start pass, and lowering in the
   tick it starts). The machinery gives every reader the game's value at the reader's
   place in the tick, one more before the lowering behaviour's own tick of that tick
   than after it, and erases the memory in the tick the game does. Tested value by value
   against a literal port of the game's per-tick decrement, for every reader's place.
4. **Entity references are keys** (B 7.1.2, "Values that are entities"): an
   `EntityTracker` is `(key, track eye height, target eye height)`, a `WalkTarget`
   `(tracker, speed as f32, close enough as i32)`, a `BlockPosTracker` a block or a point
   with its block. A player is their stay's entity key. A key is resolved each time it is
   read through the read view (`MobWorld::entity`, then `players`), never kept as a
   position. UUID memories (`liked_player`, `angry_at`, `unreachable_tongue_targets`)
   are UUIDs, as the game's; global positions are a position (the dimension is always
   the overworld until D, kept as a byte for then); the golem's two sets of positions
   are sorted lists, whose order the game never reads but in saving.
5. **The `path` memory mirrors the navigation's path**, which `MoveToTargetSink` alone
   sets (B 7.3.5) and the body keeps (`body_ids::PATH`): the memory is a mark that says
   "the navigation's path", not a second copy of the path. Named: where a navigation's
   path is replaced by something other than the sink within a tick, a reader before the
   sink's next tick sees the new path where the game sees the old.
6. **Registration is implicit, as the game's** (B 7.1.1, finding 11): the memories a
   brain has are those its sensors require and its behaviours' entry conditions name,
   computed from the type's provider (section 2.4) and never kept; setting an
   unregistered memory does nothing, reading one fails the condition, and a test per
   type checks the registered set against the brief's list for it.
7. **Sensed memories are not durable** (plan 11.2; 7.2.3): the memories a sensor's scan
   rewrites whole from the world are kept in the region's memory of the entity
   (`region/items.rs`'s `Memory`, which is dropped at every take-in), not in the record:
   `mobs`, `visible_mobs` (with its lazily decided visibility, B 7.1.1 finding 9),
   `nearest_players`, `nearest_visible_player`, the two `nearest_visible_targetable_*`,
   `nearest_visible_adult`, `tempting_player`, `is_in_water`, `nearest_attackable`,
   `nearest_visible_wanted_item` and `hurt_by` (a function of the body's last damage at
   the scan). Kept although a sensor writes them, because they outlive a scan or carry
   an expiry the game reads: `hurt_by_entity` (the hurt sensor sets it and never clears
   it but for death, B 7.2.3), `danger_detected_recently` (80 ticks after the last
   detection), and the frog's `unreachable_tongue_targets`. A test checks that every
   sensed memory is written only by sensors.
8. **What the game saves** (B 7.1.2, "The memory types with a codec") is marked in the
   table, for the fresh-load form (section 2.10).

### 2.3 Sensors and their next scan

A sensor is code in `brain/sensors.rs`, of a scan rate (20, the armadillo's scare
sensor 5) and the memories it requires (B 7.2). Its countdown is not kept: a brain keeps,
written once when it is made, **the moment it was made and each sensor's start delay**,
drawn in the provider's order from the entity's own stream at its creation
(`nextInt(scanRate)`, Brain.txt 46–319, Sensor.txt 39–48), and the sensor scans at
the brain's `max(delay, 1)`-th tick and every `scanRate` after (Sensor.txt 50–74), as a
function of now. The creation's draws come where the game's do: in `LivingEntity`'s
constructor after its yaw draw (LivingEntity.txt 338–351), before the type's own
constructor draws (the armadillo's scute time, the copper golem's first cooldown,
B 7.8.1, 7.13.1). A sensor sets the six static `TargetingConditions`' range to the
scanning mob's own follow range, never to the last scanner's anywhere in the server
(B 7.1.1, finding 9; named, as plan 7.9.7 has it). Scans read the region's land, copies
and faces (section 3) and draw nothing.

### 2.4 Activities, their requirements, and the priority structure

1. **A provider is a static table per type** (`&'static Provider`): the sensor types in
   order, and the activities in order, each an `ActivityData` (B 7.1.4, ActivityData.txt
   29–94): its behaviours with their priorities (numbered on from a start in list order,
   or explicit), its conditions, its memories to erase when it stops. Behaviours are
   static values whose arguments are constants and whose lambdas are functions;
   nothing of the provider is allocated or kept, as the goals' `Setup` is not (`ai.rs`).
2. **The brain's own state**: the core activities (`{CORE}`), the active non-core
   activity (one at a time for every type here), the default activity (IDLE), and for
   N7k's villager the schedule and its last update (B 7.1.4,
   `updateActivityFromSchedule`). `setActiveActivity`, `setActiveActivityIfPossible`,
   `setActiveActivityToFirstValid` (which changes nothing where no activity qualifies,
   B 7.0.1 point 14), `useDefaultActivity` and the erase of the stopped activity's
   memories, exactly as B 7.1.4.
3. **The order within one priority is the game's `HashMap` bucket order** (B 7.1.1,
   finding 6; Activity.txt 59–120: `hashCode` is the name's `String.hashCode`). The
   behaviours are kept as the game's `TreeMap<priority, HashMap<Activity,
   LinkedHashSet>>`: priorities ascending; within one, activities by
   `(h ^ (h >>> 16)) & 15` of their names' hashes ascending, ties in the order the
   provider added them; within an activity, insertion order. A const table gives each
   activity's bucket, and a test computes it from the names with Java's hash (it gives
   the brief's: play 0; core, rest 1; hide, admire_item 2; dig 3; meet 4; idle, pre_raid,
   celebrate, tongue 5; work, sniff 6; lay_spawn 8; emerge 10; avoid, investigate,
   roar 11; ride 13; fight, swim 14; panic, raid, play_dead, long_jump, ram 15).
   Starting walks only the active activities; the running pass walks every activity,
   active or not, in the same order, so that a behaviour outlives its activity
   (Brain.txt 911–999; B 7.1.1, finding 7). No hash map is used: the order is computed.

### 2.5 The three behaviour kinds, gates and shuffled lists

As B 7.1.6, in `brain/behaviour.rs`:

- **`Behavior`**: entry conditions, status, the moment it ends, its durations; its
  `tryStart` draws the level's `nextInt(max + 1 − min)` even where min equals max
  (Behavior.txt 63–105), `tickOrStop` ticks while not timed out and still usable, else
  stops. A behaviour that starts ticks in the same tick; one with the default
  `canStillUse` starts and stops in one tick (B 7.0.1 point 10).
- **`OneShot`** (and every `BehaviorBuilder` behaviour): its trigger runs at the start;
  it is running for the rest of the tick and stopped in the running pass. The
  declarative package is a small set of Rust helpers with the same rule: conditions
  `present`, `absent`, `registered` checked left to right, stopping at the first that
  fails, values read as of that moment (B 7.1.6); `triggerIf`, `sequence`, `point`.
- **`GateBehavior`**, `RunOne`, `TriggerGate`: children in a `ShufflingList` with
  weights; `SHUFFLED` draws one float per child in the list's current order and sorts
  by `−pow(r, 1/w)` ascending, stably (ShufflingList.txt 56–69, WeightedEntry.txt
  27–40), and the order persists until the next shuffle, so it is state; `RUN_ONE` tries
  until one starts, `TRY_ALL` tries each; a gate whose children all fail runs and stops
  in one tick, erasing its exit memories.
- **`DoNothing(min, max)`**: one draw at its start, then waits.

A behaviour's state is a serde type of its own (`type State`), as a goal's is
(`ai/selector.rs`): the status, the end moment, and its private fields (the sink's
`remainingCooldown`, `lastTargetPos` and speed; love's `spawnChildAtTime`; the look
ticker's `ticksUntilNextStart`; each `TryFind*` closure's `next`; croaking's counter,
which outlives a run, B 7.5.6; the long jump's candidates; the gate's children's states
and its list's order). The rule of counters holds here too (plan 11.2): a field that the
game counts by one every tick (croaking's counter, the armadillo's peek timer, the
golem's ticks at its chest, the long jump's preparation) is kept as a moment. The long
jump's candidate list (up to 11 × 11 × 11 positions) is not kept: it is a function of
the block the mob stood on at the start (B 7.3.16, `start`), and only the candidates
already drawn are, as a sorted list of their indices, so a preparing goat writes a few
bytes a tick for at most 20 ticks.

### 2.6 The tick, inside the mob's

`Mob.serverAiStep` (Mob.txt 1705–1815; `mobs/ai.rs`'s `server_ai_step`) calls
`customServerAiStep` after the navigation and before the controls, and every type here
begins its own with `brain.tick` and its `updateActivity` (B 7.1.5, 7.0.2 point 13). So
the skeleton gains, at that place: where the type's `brain(mob)` hook gives a provider
(none for an adult happy ghast, whose brain is never ticked, B 7.11.3), the brain's tick
on an `Ai` (with sensing's line-of-sight cache of the step, the navigation and the
controls), then the type's `update_activity`, then the type's `custom_server_ai_step` as
now. The tadpole, which has goals too, runs both, goals first (B 7.0.2 point 4). The
brain's tick is B 7.1.5's to the line: forget, sensors in list order, start pass, then
the running pass over a list taken before any behaviour ticks.

**The brain's clock is the mob's `lived`**, as the goals' and the navigation's are
(`ai.rs`, "The game time the AI measures by"): every expiry, end moment, sensor phase
and `cant_reach_walk_target_since` is a `lived`, which both regions of a line agree on
and a merge, split, lend or restore never moves. Named: ticks in which the mob lives but
its AI does not run (`NoAI`, a player steering it, dying) count down its memories, where
the game's stop (question 7.1). Game time proper (the world clock, plan 11.2) is used
only where the game's value is absolute and others read it: the camel's pose tick (the
client compares it with its own time), the copper golem's next weathering.

What a behaviour does to other entities and blocks goes into the tick's `MobOutput::local`
(`mobs/tick.rs`), applied after the mob's tick and before the next entity's, as the
same messages as across a line (plan 10.12): the partner's memories that breeding sets,
a hit, a door, a block set, an item thrown.

### 2.7 Draws and determinism

`crates/clustine-sim` stays deterministic: no hash maps, no clock, no I/O; orders are
computed (section 2.4). Every draw of B 7.1.7's four sources comes from the entity's own
stream at its place in the game's order (plan 11.2; B 10.7.1): the entity's own, the
level's on its behalf (every behaviour's start among them), each gate's own clock-seeded
list (B 7.1.1, finding 4), and the long jump's `Collections.shuffle` (the JDK's
algorithm, swapping from the end with `nextInt(i + 1)`, LongJumpToRandomPos.txt 460–503).
HotSpot's `sin`, `cos`, `atan2` and fractional `pow` in the long jump's arc, in random
directions and in the shuffle's weights are `libm`'s (plan 11.2's named list, extended
with these; B 7.1.1 finding 5). Particle draws the server makes and never sends (a
tadpole fed, a camel fed, a nautilus's bubbles every tick in water) are made and
discarded (B 7.0.2 point 11, 7.0.5 point 9). Two orders the game leaves to hashes are
fixed and named: `InteractWithDoor`'s set of doors to close (by position) and the copper
golem's walk of its 7 × 7 chunks and their chests (by chunk, then position), which decide
only exact ties (B 7.1.7, 7.13.5).

### 2.8 Durable every tick, and what that costs

The owner's rule is that every tick of an entity is durable. The brain is kept so by the
same path as the goals: the mob's tick decodes its mind's entries, runs, encodes them
again and commits only the entries that changed (`Mob::changes_from`, `region/mobs.rs`'s
`write_mob`), in that tick's commit, with its `lived` (plan 7.2.4). Nothing new is synced
to disk: the brain's bytes ride in the commit the tick already makes. What makes this
affordable is the shape of the state, not batching:

- **A brain at rest writes nothing.** Expiries, sensor phases, end moments and
  countdowns are moments (sections 2.2, 2.3, 2.5), the sensed lists are not in the
  record (2.2.7), and the path is the body's (2.2.5). A mob writes its mind only when a
  behaviour starts or stops, a memory is set or erased, or a counted field changes in a
  way the game's reader can see.
- **Copies carry no mind**, so the brain adds nothing to the copies' stream, which was
  99 % of what N7f's pen of cows sent (N7f's measurement, `bin/clustine/tests/
  herds.rs`); only its effects on the body (paths, heads) do, as for goals.
- **The cost is in the log**, per brain event: a strolling goat sets a walk target
  every few seconds, starts and stops its looker and its sink, and a gate's choice
  rewrites the gate's entry. To be measured (section 2.9), against the cow's 83 bytes a
  tick of N7f; the projection, from the events of B 7.3 for a wandering animal, is a few
  to twenty bytes a tick beyond the body's.
- **CPU**: each mob's tick decodes and re-encodes its brain's entries to find what
  changed, as the goals' are now. Measured in the same runs (the tick's length with 500
  brains); if it shows, the region keeps the decoded brain beside the record, keyed by
  the record's entries as they were, which N7j does not build unless the numbers ask.

### 2.9 The encoding, from its own measurement

The plan fixes the goals' mind and leaves the brain's form to this step, with versioned
entries so that the choice refuses no world (plan 8.7.5, 9.6.3). The brain's typed state
(memories, behaviours' states, the brain's own) is the machinery's, and the animals'
code never sees its bytes, so the animals are built while the encoding is measured.

**Candidates**, each a function from the brain's state to mind entries (ids in a range of
their own, above the goals' slots: the goals take `mind_ids::FIRST_GOAL` on, so the
brain's start at `0x4000`, the tadpole having both):

- **E1, fine**: an entry for the brain's own state (activities, creation moment, sensor
  delays), an entry for each memory present, an entry for each behaviour slot whose
  state is not a fresh one's (a slot is the behaviour's place in the provider: activity
  index and pair index, not its class, as goals' slots are). A change writes its entries
  alone.
- **E2, whole**: one entry for the whole brain. Fewest fields, every change rewrites all.
- **E3, by kind**: three entries, the memories, the behaviours, the brain's own.
- **E4, by rate**: E1's memories, the behaviours' states in one entry.

And two choices within each: an entity key as a varint or as eight fixed bytes (the keys
of what generation makes have their top bit set, plan 11.2, which postcard writes in ten
bytes), and an expiry as an absolute `lived` or as an offset from the record's `lived`.

**Measured** in two layers, with numbers from memory and from the real disk said apart
(CLAUDE.md):

1. **Bytes a mob a tick, deterministic, in the simulation** (an ignored test in
   `crates/clustine-sim`, `brain_bytes`): every candidate run side by side on the same
   brains, by encoding each tick's brain in each and diffing it against the last, as the
   region's `changes_from` does, and counting what the store's log codec
   (`clustine-format`) writes of the mind's fields; the body's bytes beside, for the
   comparison with N7f's 83. Scenarios of 20 mobs for 6000 ticks each: goats on a slope,
   with no player and with a player holding wheat; frogs in a swamp pool; axolotls with
   tropical fish; armadillos with a bot sprinting past every 200 ticks; camels on sand;
   sniffers on grass; allays with an item, items about and a note block; copper golems
   with two copper chests and three chests; tadpoles; nautiluses with a pufferfish;
   ghastlings with a player. Reported for each: the mean and 99th percentile of mind
   bytes a mob a tick, the share of ticks that write the mind, the fields a write
   makes, and the encode-and-diff time a mob a tick.
2. **A divided world, end to end** (`bin/clustine/tests/brains.rs`, ignored, as
   `herds.rs`): a pen of 100 brain animals (25 each of goats, armadillos, frogs and
   camels) at a pinned line for a minute; 20 axolotls hunting fish across the line; 20
   goats following wheat across it; a merge with 100 of them; a crash's restore of a
   region of 500. The world's growth a minute, the copies' updates, the tick's length,
   and that every animal's brain is as it was after the merge and the restore.

**Chosen** by these rules, in this order, and written into the roadmap with the numbers:
a candidate that writes the mind of a penned, unwatched animal in more than one tick in
ten is out (the hard requirement of 2.8); of the rest, the fewest mean mind bytes a mob a
tick over all the scenarios, each weighted alike; a simpler candidate (fewer kinds of
entry: E2, then E3, E4, E1) within ten per cent of the smallest is taken instead; the
key and expiry choices by the same rule. The other candidates are then removed from the
code. Layer 2 runs again with the choice, and N7j's "done" in the roadmap states its
world growth beside the cows'.

### 2.10 Fresh loads

The plan matches the game's own reload where nobody can see it (plan 7.2.7, 8.2, 9.1,
10.1): at a fresh take-in a brain is made again from the memories the game saves, with
their expiries, every sensor's delay drawn again, every behaviour stopped and every
unsaved field of the type reset (the sniffer idle with no seed tick, a copper golem with
no first cooldown, B 7.0.5 point 2, a camel's dash cooldown 0, an armadillo's state
ticks 0, an allay not dancing, a ghast's leash-holder time 0). Each type's module gets
that form as its `fresh_load` hook and the machinery a `Brain::fresh_load`, each with a
test from the brief's lists. **Found at `f4a3334`**: the fresh case (`Took`, the fresh
and continued marks of plan 9.1 and 10.3) is not in the code; `take_up_entities` treats
every take-in alike, so goals and brains are kept whole even where the game would reload
(question 7.2).

## 3. What reads across a line

The rules are plan 11.2 and 7.5: a mob reads its region's land, its copies (one chunk
beyond the land) and the faces of other regions' players in those chunks; blocks beyond
read as an unloaded chunk's (air, and a path search's end); entities beyond are not
there; a remembered thing beyond answers "unknown", and the memory is kept; what it does
to another region's entity or block is an entity or block effect. Per piece (B 11.7):

1. **Sensors.** `mobs` and `visible_mobs` within the follow range: through the copies,
   but for the sliver of a second chunk that a wide mob's box reaches (a camel 0.85 of a
   block, a sniffer 0.95, a happy ghast 2: B 11.7.1), which no copy holds: named, and
   question 7.8. Players (`nearest_players`, temptation within 10, the ghastling's 16,
   the armadillo's scare box of 7, 2, 7) from the region's own and the faces; the faces
   gain **sprinting** (the scare reads it; `api::PlayerFace`, the store's face, the
   transfer), the lead's. Items within 32 (the allay's sensor) only from land and
   copies (named: an allay near a line does not see items beyond the next chunk).
2. **An entity a memory names** resolves through the read view; where it does not
   (beyond the copies), the memory is kept and its reader acts as if it were not there
   this tick: a look target's visibility test fails as the game's would for one out of
   sight, and a walk target to it makes no new path. The allay's liked player is the
   exception the plan already makes for pets: the world's players, published once a
   second (`MobWorld::player_anywhere`), within 64 blocks, a second late.
3. **The allay's liked note block** up to 1024 blocks away: where its chunk is neither
   land nor copy the answer is "unknown": the memory is kept, and that tick the allay
   takes its liked player as its target, as it would were the note block not liked
   (question 7.11). Hearing: the allay's two listeners are entity listeners that move
   with it (B 7.10.6, `DynamicGameEventListener`), which `region/vibrations.rs` gains;
   an event in another region is not heard, as a sculk sensor's is not (N6's named
   difference).
4. **Breeding** (`AnimalMakeLove`, B 7.3.9): within a region the initiator writes the
   partner's `breed_target`, `look_target` and `walk_target` every tick as local
   writes, exactly the game's. With a partner only a copy shows, the region-breeding
   protocol of the farm animals holds (`region/breeding.rs`, plan 9.4 and 10.7): both
   run their own `AnimalMakeLove` towards each other (neither sees the other's writes),
   and at the child's moment only the higher key asks; the partner's region breeds as
   the game's `this` (a frog becomes pregnant, a sniffer lays its egg, a child is made
   there), and the asker's love and age reset by the answer. The "asked" mark is the
   animal's type state, which copies carry. A partner's panic across a line is not read
   (the mind is not copied), as for the farm animals.
5. **Hits a brain deals** (the goat's ram with its knockback, B 7.3.19; the axolotl's
   melee, B 7.3.20; the nautilus's charge, B 7.12.3): within a region as `Local::Hurt`,
   which gains a knockback; across a line as a new entity effect, `EntityUse::MobHit`
   (the attacker, its type, the damage, the knockback), the hit ADR-0028 section 7
   already lists and N7h's wolves and N7i's pufferfish wait for (question 7.4).
6. **The nautilus's anger** names a UUID it looks up world-wide (B 7.0.5 point 10): it
   is looked for among the region's own and copied entities and players; a miss is the
   game's "no target".
7. **Blocks a behaviour sets**: frogspawn up to a block across a line goes as the
   `SetIf` block effect (air to frogspawn), its game event and sound in the holder; a
   door `InteractWithDoor` would open in another region's land is not opened (named:
   a door exactly at a line; question 7.5 covers the golem's doors too).
8. **Long reaches**: the long jump's box (the goat's 5, the frog's 4 and 2), the
   sniffer's dig spots (up to 18 across, paths to them), the happy ghast's careful sweep
   (up to 17 blocks), its good-target test (15 along each axis) and its home (up to 64):
   blocks beyond the copies read as unloaded; for the ghast's sweep an unknown block is
   not passable, so it waits and picks another point (B 11.7.4's proposal). Named:
   these plan shorter near a line (plan 11.6).
9. **The copper golem's chests** in its 7 × 7 chunks (B 7.0.5 point 5, 11.7.5): it
   searches only the chests of its region's land, and so never walks to, opens or fills
   a chest of a copy; it counts as an opener of its chest through the containers'
   recheck, which gains entity openers (`region/containers.rs`, the lead's); queuing
   reads its chest's openers, which are its region's. Named for a pinned line: a golem
   ignores chests across it. Where regions follow players, N7n's merge for work at a line
   counts a golem whose 32-block reach meets a line (question 7.5).
10. **Moving across**: a jump, a dash, a charge or a flight that crosses a line is an
    arrival with the mob's record whole, its mind included (ADR-0028 section 5;
    `LONG_JUMP_MID_JUMP`, the pose and the jump's motion travel with it, B 11.7.3). A
    camel with two players, a happy ghast with four and a nautilus with one cross as one
    unit with their riders (N5's units; section 4's riding part).
11. **Game time**: the camel's pose tick is the world clock's, so a sitting camel across
    a line may read its pose up to a second off (plan 11.6).

## 4. The contracts first, then the parts

### 4.1 N7j1, the contracts (the lead's), committed and pushed before anything is delegated

Each with a test, as N7a1 was (plan 11.3):

1. **The brain's types and interfaces** (`mobs/brain.rs`): `MemoryKind` and the memory
   table (ids, value types, sensed, saved by the game), `Slot`, the typed `Memories`
   API (`get`, `set`, `set_with_expiry`, `erase`, `expires_in`, `check(status)`,
   registered); `Provider`, `ActivityData`, the activity table with its buckets;
   `Behaviour` (`State`, `required`, `try_start`, `tick_or_stop`, `do_stop`) and its
   three kinds' helpers; `Sensor`; the `Brain` with its own state. The tick's core
   (section 2.6: forget, sensors, start pass, running pass) is in the contract, since
   every part's order rests on it; the shared behaviours and sensors are not.
2. **The mind's ids** for the brain (`mind_ids`), and the encoding behind one function
   pair (`encode`, `decode`), with the four candidates of 2.9 behind a test-only switch.
3. **The hooks** in `MobType` (`mobs/types.rs`): `brain(&Mob) -> Option<&'static
   Provider>`, `update_activity(&mut Brain, ...)`, `fresh_load(&mut Mob)`, and those the
   parts need that touch shared files: `wants_to_pick_up` and `picked_up` for
   `Mob.aiStep`'s pick-up loop (the allay's, B 7.10.4; N7i's fox shares it),
   `equipment_changed` (the allay's rule, B 7.10.5), `can_be_collided_with` (the still
   happy ghast, B 7.11.4), `requires_precise_position`. The skeleton's call of the brain
   at `customServerAiStep`'s place (`mobs/ai.rs`).
4. **The dispatch**: the fourteen modules as empty files the parts fill, declared in
   `types.rs`, and their arms in `types::built` written but left out until the lead
   merges each part (an egg of a type not yet built makes nothing, plan 11.6), so that
   no part edits `types.rs`. Tests that use the goat as a mob of no type of its own
   (`shown.rs`'s test, `region/tests/mob_bodies.rs`) move to a test-only plain type.
5. **Local writes and effects**: `Local::SetMemories` (a partner's memories, by key),
   `Local::Hurt` with a knockback, `Local::OpenDoor { pos, open }` (`DoorBlock.setOpen`
   with its sound and game event), `Local::Tongue { prey, velocity, amount }` if
   question 7.3 is taken, `Local::ContainerTake` and `Local::ContainerPut` (the golem's,
   on its region's own chest, with the openers' count); `EntityUse::MobHit` in
   `clustine-world/src/effects.rs` and its taking in the holder.
6. **The region's sides that are shared files**: the sensed lists in the entity's
   memory (`region/items.rs`'s `Memory`); entity openers in the containers' recheck
   (`region/containers.rs`); entity game-event listeners that move with their entity
   (`region/vibrations.rs`); the scheduled-tick and random-tick arms for frogspawn, the
   sniffer egg and the dried ghast (`region/ticks.rs`, `region/random_ticks.rs`), each to
   a function the parts write; the carved pumpkin's golem patterns (the copper golem's
   now, the snow and iron golems' in N7k) as one hook in its `onPlace`.
7. **What clients see** (`shown.rs`): `DataShown::Serialised(Serialiser, i32)` for the
   armadillo's, sniffer's, copper golem's and weathering states, `Holder::FrogVariant`
   and `Holder::ZombieNautilusVariant`, `DataShown::OptionalEntity(Option<EntityId>)`
   for the frog's tongue target; the entity events these types send (`Event`); and the
   edge's side of each (section 5).
8. **The player's face** gains `sprinting` (section 3, item 1), in the region, the
   store's face record and the transfer.
9. **The extraction**: the loot tables `gameplay/armadillo_shed`,
   `gameplay/sniffer_digging` and `brush/armadillo` with their random sequences, the
   `instrument` registry and its two goat tags, the `frog_variant` and
   `zombie_nautilus_variant` registries, the entity, block and item tags the brief names
   (`followable_friendly_mobs`, `frog_food`, `axolotl_hunt_targets`,
   `axolotl_always_hostiles`, `nautilus_hostiles`, `snaps_goat_horn`,
   `sniffer_diggable_block`, `happy_ghast_avoids`, `mob_interactable_doors`, `copper`,
   `copper_chests` and the rest), and the `memory_module_type` registry's names for the
   table's test.

### 4.2 The parts

Each delegated part has files of its own and a worktree of its own (with its own
`CARGO_TARGET_DIR`, removed when merged), starts from `main` after N7j1 is pushed, and
does not commit to `main`, push, download or start Docker, kind or the official server.
Animals' parts begin at once on everything but their brains' activities (the body, the
type state, the synced data, `finalizeSpawn` and its draws, interactions, buckets,
blocks, their own behaviours against the contract's `Behaviour`), and wire their
providers once part A is on `main` (`git merge --ff-only main`).

| Part | Who | Files of its own | Brief |
|---|---|---|---|
| **A** sensors and shared behaviours | delegated | `mobs/brain/sensors.rs`, `mobs/brain/behaviours/*.rs`, their tests | 7.1.3, 7.2, 7.3, 10.7.1 |
| **B** axolotl, frog, tadpole | delegated | `mobs/types/axolotl.rs`, `frog.rs`, `tadpole.rs`, `amphibian_tests.rs`; `region/frogspawn.rs`; `region/tests/amphibians.rs` | 7.4, 7.5, 9.7.2, 10.7.2 |
| **C** goat, camel, camel husk, armadillo | delegated | `mobs/types/goat.rs`, `camel.rs`, `camel_husk.rs`, `armadillo.rs`, `highland_tests.rs`; `region/tests/highland.rs` | 7.6 to 7.8, 9.7.3, 10.7.3 |
| **D** sniffer, allay, happy ghast | delegated | `mobs/types/sniffer.rs`, `allay.rs`, `happy_ghast.rs`, `flier_tests.rs`; `region/sniffer_eggs.rs`, `region/dried_ghasts.rs`; `region/tests/fliers.rs` | 7.9 to 7.11, 9.7.4, 10.7.4 |
| **E** nautilus, zombie nautilus, copper golem | delegated | `mobs/types/nautilus.rs`, `zombie_nautilus.rs`, `copper_golem.rs`, `keeper_tests.rs`; `region/copper_golems.rs`; `region/tests/keepers.rs` | 7.12, 7.13, 9.7.5, 10.7.5 |
| **M** the measurement and the encoding | the lead | `brain/encoding.rs`; the `brain_bytes` test; `bin/clustine/tests/brains.rs` | section 2.9 |
| **R** riding the camel, the nautilus and the happy ghast | the lead | `region/riding.rs`, `riders.rs`, `physics/vehicle_move.rs` (N5's and N7h's) | 7.7.5, 7.11.4, 7.12.5 |
| **X** the edge, the comparisons, what to try | the lead | `services/edge`, `bin/clustine/tests/mobs_oracle.rs`, the roadmap | sections 5, 6 |
| **S** tests from the specification | delegated, to someone who built none of it | `crates/clustine-sim/tests/brains_specification.rs` (and a second file if it grows past one) | 7, 9.7, 10.7, 11.7; this plan |

The camel is `AbstractHorse`'s (B 7.7): part C reads `mobs/types/equine.rs` and may
not change it; what it needs that is not there (the camel's own seat and dimensions, its
`isTamed` always true, no rearing) it overrides in `camel.rs`, and anything the horse
family's code must call it asks the lead for. A type's own behaviours (B 7.3.24) and
controls (the axolotl's and frog's controls, the ghast's move control and random float
goal, the frog's node evaluator) are in its own file; `ai/navigation.rs`'s evaluator
enum gains the frog's variant in N7j1, its code in part B's file.

**Riding (part R) is the lead's**, as N7h's was ("riding mine"): the hand-over of a unit
with players is where ordering mistakes hide (CLAUDE.md). Parts C, D and E expose what
the riding needs as plain functions, as N7h's horse part did (`n7h-riding-plan.md`,
section 7): whether a player on it steers, its seats as functions of its pose (the
camel's `getBodyAnchorAnimationYOffset`, B 7.7.5), its ridden input, rotation and speed,
its jump or dash and cooldown, its dismount place, the ghast's still timeout and
collision. R then plans them in a note of its own in the manner of the riding plan,
reviewed, before it is built: the camel's two riders and dash, the nautilus's dash under
water, the happy ghast's four players, flight, staying still and being stood on.

### 4.3 What each part brings, and how it is verified

- **A**: a test per behaviour of its game rules on a small test brain (`ai/tests.rs`'s
  `Plot` world): the start-then-tick and one-shot-stop rules, a behaviour outliving its
  activity, memory expiry to the tick, every countdown's value at every reader's place,
  the sensors' phases (delay 0 and 1 scan at the first tick), the bucket order, the
  no-change rule of `setActiveActivityToFirstValid`, gates in both policies and both
  orders, the shuffle's weights by their distribution and its draws by count, the sink's
  cooldown and `cant_reach_walk_target_since`, panic re-arming, the `TryFind*` waits
  and their Manhattan order (B 7.3.13, the order **Inferred**), the long jump's
  candidates derived from the start block, the JDK shuffle, the ram's start positions,
  the implicit registration.
- **B to E**: a module's tests (the types' `*_tests.rs`) of its type state, synced data
  with indices and defaults (B 9.7), `finalizeSpawn`'s draws in order (B 10.7), each
  interaction, sounds, variants and the fresh-load form; region tests
  (`region/tests/*.rs`) of each behaviour as a player meets it (section 1: an armadillo
  rolled up by a sprinting player and unrolled 81 ticks after; a goat's jump landing; a
  frog laying spawn and its hatching; a sniffer's seed dropped 120 ticks into its dig; an
  allay fetching to its player and to a note block; a golem moving 16 items with its
  lid's events and waiting while a player has the chest open; a ghastling growing up into
  goals; a nautilus tamed); and across a pinned line, for each type: tempted across it,
  breeding across it by the protocol, a jump or charge across it arriving with its brain
  whole, and a merge, a split and a restore keeping a panic, a long jump and a dig.
- **M**: the encoding's round trip in every candidate, and the measurement of 2.9.
- **Each part is verified as it will be pushed**: in a clean worktree with its own
  `CARGO_TARGET_DIR`, the tests' worlds in memory (`TMPDIR=/dev/shm/...`), the four
  commands of CLAUDE.md judged by their exit codes (`cargo fmt --all --check`, `cargo
  clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace
  --locked`, `CLUSTINE_TEST_PINS=0,4 cargo test -p clustine --locked`). The lead reads
  each part's code and tests, runs them again, and merges one verified commit a part to
  `main`.

### 4.4 Tests from the specification (part S)

Written after B to E are on `main`, from the brief's section 7 with 9.7, 10.7 and 11.7,
this plan and the bytecode, against the public API alone, by someone who wrote none of
the code, in the manner of `crates/clustine-sim/tests/water_animals_specification.rs`:
the machinery's order (a behaviour's first tick, the activities at one priority, an
expiry's last tick), each animal's rules as a player meets them, the draws' counts where
an order is fixed, and the line's cases (section 3). Where they find a defect, it is
mended with a test of its own, and the roadmap says what they found.

### 4.5 Order

N7j1 (contracts) → A, and B to E on their brain-free parts, in parallel → B to E's
brains → M (measurement and choice), which needs A and the first animals, and runs
beside the rest → R (riding, after C, D and E expose its hooks) and X (the edge's side
with each part, the comparisons after) → S → the roadmap's "what to try", whose items
follow section 1, with a pinned line's cases.

## 5. The edge's part

The worker never sees a packet (ADR-0005): each type's module gives its synced values as
`DataShown` (`shown.rs`), its entity events and sounds as `mobs::body::Event`, and the
edge maps them, as it maps every mob's now (`services/edge/src/tracking.rs`,
`living_values`).

- **Synced data** (B 9.7): the serialisers new to the edge, by their registration ids
  (`EntityDataSerializers`): `ARMADILLO_STATE`, `SNIFFER_STATE`, `COPPER_GOLEM_STATE`,
  `WEATHERING_COPPER_STATE` (each a VarInt of the state's id), `FROG_VARIANT` and
  `ZOMBIE_NAUTILUS_VARIANT` (registry holders, their ids from the synced registries as
  the cow's are), `OPTIONAL_UNSIGNED_INT`; `LONG` (the camel's pose tick) and `POSE`
  (croaking, using the tongue, long jumping, sitting, digging's box) exist. Every index
  and default by B 9.7's tables, and each type's `shown` says which are defaults.
- **The frog's tongue target** names an entity: the region gives its key
  (`DataShown::OptionalEntity`), and the edge writes its own wire id for that key, which
  every connection of the edge shares (N1), or an empty value where the edge has none;
  where a wire id for that key is made later while the value holds, the frog's data is
  sent again. Tested at the edge (`tracking.rs`'s tests): a target with a wire id, one
  without, and one that gets one.
- **Entity events**: 58 and 59 (the goat's head), 63 (the sniffer starting to dig), 64
  (the armadillo's peek), 18 (an allay's duplication), 6 and 7 (taming's smoke and
  hearts), through the existing `entity_event`.
- **Sounds that leave out the player who used the mob** (the allay's chime, item given
  and taken; a goat's milking; B 7.0.4 point 13): `Did::Sound`'s `but_the_player`, as now.
- **The happy ghast**: the exact position packet as it begins to stay still
  (`ClientboundEntityPositionSyncPacket`, B 7.11.4) and absolute moves while it stays
  (`setRequiresPrecisePosition`), as flags on its report; its four passengers by N5's
  `set_passengers`.
- **Menus**: the camel's and the nautilus's saddle screens are the horse's
  `mount_screen_open` with the mob's wire id (N7h), the nautilus's of no slots
  (B 7.12.1).
- **Precise ids in effects**: the copper golem's chest lid events and sounds and the
  doors' are block events and sounds the edge already sends.

## 6. Comparisons with the official server

In the manner of `bin/clustine/tests/mobs_oracle.rs`: a builder bot does the same on
both servers beside the spawn of a flat world (water poured where it is needed), a
watcher records what it is sent, and the cases hold both to what the rules fix whatever
the draws. Run on a quiet machine with `--no-fail-fast` (CLAUDE.md). The lead's, part X.

1. **Each type from its egg** (all fourteen): the spawn packet and its order, every
   synced value and which are sent, the attributes, the head facing the yaw, the
   equipment; a baby where the egg is used on an adult.
2. **The armadillo rolled up by a sprinting bot**: state 1, then 2 eleven ticks later,
   the roll and land sounds, and back to 0 with the unroll sounds once the bot is gone
   (timed against the 80-tick memory: 81 ticks after the last scan that saw it, within
   the scan's five ticks); brushing's scute and sound.
3. **A goat's long jump and a frog's croak**: the poses (6, then 0; 8 for 60 ticks),
   the jump's sounds, the landing's volume of 2. When they happen is drawn; their order
   and lengths are not.
4. **A sitting camel**: the pose and the pose tick, negative while sitting, compared
   against the time each server sends; standing up by a rider's forward key once the
   riding is built.
5. **A sniffer's dig**: the state sequence (scenting, sniffing, searching, digging,
   rising, happy, idle), event 63, the seed tick of 120 after digging begins and the
   item at the head block's corner.
6. **An allay given an item and taking one**: the equipment packet, the sounds sent to
   the watcher and not to the giver, the pick-up animation, the item thrown at the
   player.
7. **A copper golem built from a pumpkin** on a copper block: the block changes (both
   to air, then the copper chest), the spawn and its sound; at a copper chest with items,
   the state at tick 1, the sound at tick 9, the lid's block events, then the chest.
8. **Buckets**: an axolotl and a tadpole taken (the sounds, the removal) and poured.
9. **Frogspawn hatching** after a laying: its block, its hatch sound, two to five
   tadpoles at its block.
10. **A happy ghast**: a harness put on, a player mounted, the stay-still flag and the
    exact position packet when a second player stands on it, the goggles' sounds (with
    part R).

A probe first (`probe_...`, as `mobs_oracle.rs`'s rider probe) where a rule above is
read with **Inferred**: the `TryFind*` Manhattan order (B 7.3.13) can be probed with
water placed at two positions of one distance in front of an axolotl.

## 7. Open questions, each with a recommendation

1. **The brain's clock** (2.6). Recommended: the mob's `lived`, as the goals use, named
   for the ticks without AI (a camel or nautilus ridden for a minute comes off with its
   cooldowns that much shorter). The alternative keeps an offset of the AI-less ticks,
   written at each change of AI, exact at the cost of a write at each ride.
2. **Fresh loads** (2.10). Recommended: build every fresh-load form now with its tests,
   and keep every take-in as it is in the code (nothing reset, named as the plan's 11.6
   names it for the cases it chose) until the lead brings `Took`; or bring `Took` into
   N7j1, which touches the store's commit thread (plan 9.1, 10.3) and is the lead's.
3. **The frog's tongue** (1.2). N7k brings slimes and magma cubes as bodies from eggs,
   which a frog would eat in the game. Recommended: build `ShootTongue` in N7j within a
   region (`Local::Tongue`: the velocity set, the hit, the removal if dead, the edge's id
   translation), so that it works as soon as N7k's slimes come; across a line a frog
   takes no prey that only a copy shows (named) until N9, where the tongue's effect is
   planned with its prey (plan 7.5.3).
4. **A mob's hit across a line** (3.5). Recommended: `EntityUse::MobHit` in N7j1, since
   three of this step's types and N7h's wolves need it and ADR-0028 section 7 lists it;
   the alternative leaves it to N7n and names a goat ramming nothing across a pinned line.
5. **The copper golem at a line** (3.9). Recommended: its region's chests only (a pinned
   line's named difference), and in N7n a golem whose reach of 32 blocks meets a line of
   regions that follow players counts as work at that line, merging them. A door the golem
   meets in another region's land stays shut (named). The alternatives, a chest effect
   with a condition on the slot, or wider copies, cost a design of their own.
6. **Which memories are sensed and not kept** (2.2.7). Recommended: the list given,
   re-sensed at every take-in; the review should look at each, as a memory wrongly
   called sensed loses state at a line.
7. **The encoding's choice rule** (2.9): the hard requirement, the fewest bytes, the
   simpler within ten per cent. Recommended as stated; the owner may weigh the real
   disk's growth rather than the log's bytes.
8. **Wide mobs' sliver** (3.1). Recommended: accept it, named (a camel, sniffer or happy
   ghast at a line misses entities in the last 0.85 to 2 blocks of the second chunk
   across); widening copies for entities is a change of ADR-0025's copies for all.
9. **Riding as the lead's own note** (4.2, part R), reviewed before it is built, with the
   happy ghast's four players as N5's unit of several player seats. Recommended; the
   review should say whether N5's unit holds four player seats as it holds a boat's two.
10. **Item components** (1.2): plain goat horns, fresh buckets, nameless statues until
    the step of item components the owner agreed to after N7 (plan 11.7, question 4).
    Recommended, as before.
11. **The allay's liked note block beyond the land** (3.3): kept, and the liked player
    taken for that tick. Recommended; the game never has the case.
12. **`sprinting` in the face** (3.1), which changes the store's face record; the second
    half refuses older worlds at N7l anyway (plan 11.7, question 7). Recommended.
13. **The goat's item sensor**, whose memory nothing reads (B 7.6.0): ported, as the
    game scans and the scan draws nothing. Recommended, for the tick's cost to be the
    game's.
14. **The villager's machinery** (B 7.14.2): N7j's machinery includes what the
    villager's core and idle packages need (`InteractWithDoor`, the look by category,
    the schedule's update and `refreshBrain`), tested here, so that N7k builds the
    villager without changing it. Recommended.
15. **The mob's pick-up loop** (4.1, item 3), which N7i's fox shares: in N7j1 if N7i's
    wild animals have not brought it first. Recommended.

## 8. The review (2026-10-21), and what the plan is now

An independent review checked the activities' hash buckets for all 26 names (each
matches), the shuffled lists, the draws, the sensors' delays, the tick's place, the brain's
clock, the sensed lists' members, section 3 and the division of work, and found them
sound. It found eleven things to change; the plan stands amended by them:

1. **The re-sense after a take-in is the lead's, in N7j1, and is specified** (2.2.7, Q6).
   Sensors scan every 20 ticks (the armadillo's every 5) and behaviours read their lists
   every tick between, so lists dropped at a take-in and left empty until the next scan
   would stop a goat tempted across a line for five seconds (`FollowTemptation`'s stop
   and its cooldown of 100). The re-sense runs at the mob's first brain tick after a
   take-in, after the forget pass and before the start pass; its sensors run in list
   order and write only sensed memories (the mob sensor is passed over; the hurt-by
   sensor writes only `hurt_by`); it makes no draws and moves no sensor's phase. Named:
   the lists are the take-in's world, not the last scan's. Tested: an arrival in the
   middle of a temptation keeps following and starts no cooldown.
2. **The body keeps the last damage source** (3.1): its type id, its cause's key and its
   game-time stamp, written at each hurt (already a write), so that the hurt-by sensor
   judges `#panic_causes` and `#panic_environmental_causes` and a mob on fire, in lava or
   on a cactus panics, as the armadillo's and every brain animal's panic need.
3. **A provider names its memories** (2.2.6, 2.4.1): the allay's, the sniffer's and the
   nautiluses' use the three-argument `Brain.provider`, whose memories are registered
   first (Brain.txt 67–103). Without them the allay's liked player, the sniffer's explored
   positions and the nautilus's anger would be dropped. The test of each type's
   registration compares with the providers' lists in the dumps.
4. **A counter lowered on some ticks only is kept as (value, the moment it resumed)**
   (2.5, 2.8), written when its counting pauses or resumes, not at every step:
   `SetEntityLookTargetSometimes`' `ticksUntilNextStart`, `TryFind*`'s `next`, and
   `MoveToTargetSink.remainingCooldown`, each named and its writes counted by a test. A
   goat in a pen would otherwise write its mind on four ticks in ten for the first alone.
5. **The rule of one write in ten ticks checks the state's design, not the encoding**
   (2.9): it must pass before the candidates are compared, and where it fails the state
   changes. The expiry as an offset from `lived` is dropped (it changes every tick). Each
   type has its own penned, unwatched case; bytes are counted after zstd, by the format's
   codec as a dev-dependency of the sim; layer 2 leaves the sensed lists out. The bytes
   rule and the 10 % rule of simplicity stand.
6. **Writes and reads outside the brain's tick are defined against the record's `lived`**
   (2.2.2, 2.2.3): a value set outside counts from the mob's next forget pass, in this
   server tick if the mob has not yet ticked and the next otherwise, as the game's; an
   absent memory's time to expiry is `i64::MAX` (`Long.MAX_VALUE`, which the armadillo's
   balling up reads); a cooldown set again while it runs restarts. The `Memories` API of
   N7j1 says so, tested in the start pass, the running pass, a hurt before and after the
   mob's tick and an interaction. 2.2.2's bucket example goes: buckets carry nothing yet.
7. **N7j1's contracts gain** (4.1, 4.2): a breeding hook that gives mind changes and
   outputs, not only a type state (a frog's pregnancy, a sniffer's egg), with
   `AnimalMakeLove` wired to the breeding ask across a line; the extension points of the
   behaviours types extend (the camel's, armadillo's and sniffer's panics, the
   armadillo's walk sink, the frog's long jump), fixed before parts A, C and D begin; a
   `memory_of(&Entity, kind)` free of the encoding, "unknown" for a copy; the tick core's
   pass and slot, which a cooldown's value at its reader's place needs; the re-sense of
   item 1; the fox's pick-up lifted into the shared hook, the lead editing `fox.rs`; the
   allay's vibration in flight kept in its record as a stamp. The path's evaluator is in
   `ai/pathfinder.rs`.
8. **"Unknown" never counts as dead, removed or in another level** (3.2, 3.6). For each
   reader that erases on "not there" (the hurt-by sensor, `StopAttackingIfTargetInvalid`,
   `AnimalMakeLove.canStillUse`, `LookAtTargetSink`, `MoveToTargetSink`'s start) the plan
   says whether it keeps and skips or takes the game's out-of-sight result, reader by
   reader, so that an axolotl hit by a player who walks over a line remembers them. A
   copy's path for `InteractWithDoor` is read from the copy's body.
9. **The long jump keeps** its chosen jump, its preparation's start (a stamp; 40 ticks),
   its initial position (compared exactly) and its tries, and the frog's
   `currentlyWantingPreferredOnes`; the drawn candidates' indices go, being read only in
   the tick that draws them.
10. **The happy ghast's leashes are built** (part D, with R): it holds boats and animals
    on quad leashes and shows `IS_LEASH_HOLDER`, which a player tries within minutes.
11. Named in what to try: a goat horn blows nothing until instruments come (agreed), and
    `VillagerCalmDown` will erase `hurt_by` in N10, which the test of sensed-only writes
    must allow then.
