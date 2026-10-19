# Step N7: living entities, animals and the structures' entities, planned

Draft of 2026-10-19, revised the same day after independent reviews (sections 7 and 8,
each overriding what it amends above it): two reviews of the draft are answered in section
7, a third review of section 7 in section 8, which also cuts the step into two halves, and
a fourth review of section 8 and the whole in section 9, and a fifth of section 9 and of
the plan then stated in section 10. **Section 11 states the plan as it stands**; the
sections before it are its reasons. Step N7 of the
[phase plan](n-plan.md): "Living entities and animals: health, attributes, damage, death
and loot, knockback; players hitting mobs (interact); spawn eggs; goals, controls and
path finding; the animals, breeding and ageing; animals made with the chunk; the
structures' entities placed at generation (villagers, illagers, guardians, …) with their
simple goals, the villagers' brain left to N10. Measured: the log and the tick's length
with many mobs." The game's rules are in the [porting brief](n7-brief.md), read from the
26.3 jar by eight readers over 1,036 classes; this plan cites it by section (brief 2.0.4
is finding 4 of section 2) and decides what it leaves open.

## 1. What a player meets after N7

- Plains, forests, savannas, taigas, jungles, deserts, swamps, beaches, snowy lands and
  the mountains have the animals the game makes with their chunks, exactly where the game
  puts them, with the same colours, variants, babies and horse stats (brief 8.0.2): cows,
  pigs, sheep, chickens, horses, donkeys, rabbits, wolves, foxes, llamas, goats, cats,
  ocelots, parrots, pandas, polar bears, turtles, frogs, armadillos and camels. Villages have their villagers, cats, horses, sheep, cows, pigs and
  their iron golem; outposts their caged allays and golem; igloos their clerics; bastions
  their piglins (bodies only, N9 and the Nether).
- Animals stroll, look at players, look around, float, panic when hit, follow a player
  holding their food, breed when fed, have babies that follow their parents and grow up,
  and eat grass; sheep can be sheared and dyed, cows and goats milked, mooshrooms milked
  for stew and sheared, chickens lay eggs, pigs and horses ridden (a saddle, a carrot on
  a stick), horses, donkeys, mules and llamas tamed and given chests, wolves and cats
  tamed, sitting and following their owner, parrots on a shoulder, bees going between
  their nest and flowers, turtles going home to lay eggs, foxes sleeping by day and
  carrying things, axolotls playing dead, frogs leaping, camels dashing with two riders,
  armadillos rolling up, allays fetching, a happy ghast carrying four players, copper
  golems sorting chests.
- A creative player's hit hurts a mob by the game's survival rules (cooldown, critical
  hits, sweeps, knockback: brief 1.0.4); a mob dies after 20 ticks with its loot and its
  experience orbs, which the player picks up (brief 1.0.5). Spawn eggs make every mob, on
  blocks, in water and on a mob of their kind (a baby). Name tags and leads work on mobs.
- Water has nothing in it until N9's natural spawning (fish, squid, dolphins, glow squid,
  bats and axolotls are made only by natural spawning: brief 8.0.5), but every one of
  them comes from its spawn egg with its AI.
- The day passes: the sun moves, night falls (decision 6); there are no monsters at night
  until N9.
- A second player sees all of it the same; across a pinned line an animal follows wheat
  held on the other side and a dog follows its owner over it (decision 4).

## 2. Decisions the brief leaves open (taken here; the owner can overrule)

1. **What N7 ports.** Every mob whose category is a creature's (`creature`,
   `water_creature`, `water_ambient`, `underground_water_creature`, `axolotls`,
   `ambient`), with its AI, its interactions and its spawning with the chunk where the
   game has one; the three golems; the villager and the wandering trader with a reduced
   brain (decision 9); every other mob (monsters, illagers, piglins, guardians, the
   witch, the zombie villager) as a body with the goals and behaviours that never target
   anything, so that a spawn egg or a structure makes it and it strolls, looks and
   floats, and N9 adds what attacks (decision 10). The combat side of animals (a wolf
   hunting a sheep, a fox a chicken, an axolotl a fish, a golem a monster) comes in N7
   for the targets N7 has, since none of it targets a creative player (brief 2.0.1,
   7.0.1.9).
2. **The AI's state is the entity's, durable, and is never reset by Clustine.** The game
   saves no goal, lock, control, path, target or brain memory without a codec (brief
   2.0.5, 3.0.2, 7.0.1.1), and loses them at every chunk load. Here a crossing, merge,
   split, lend, return and restore would do so in ordinary play: a cow walking across a
   line would stop dead, a calf stop following its mother, a dog forget its owner's
   command. So the entity's record carries all of it: which goals run and hold which
   flags, each goal's fields, the controls' wanted positions and operations, the
   navigation's path (nodes and the next index), its timers and the malus changes goals
   made, the brain's memories (entity references as keys), sensors' next scans and
   running behaviours with their end ticks. A restore is no reload either: a crash loses
   no tick of any mob (the owner's answer 1). Nor is a chunk taken up again after nobody
   held it, where the game would have unloaded and reloaded it: its mobs carry on as
   they were (named: in the game they start their goals again, a panic ends, a brain
   keeps only its saved memories), so that one rule holds everywhere.
3. **Counters.** What the game counts by one every tick of the entity (`ambientSoundTime`,
   `noActionTime`, `headStableTime`, an age, a memory's time to live, a sensor's
   countdown, `CountDownCooldownTicks`' memories, a goal's `adjustedTickDelay` timer) is
   kept as the entity's tick count at which it was last set and read as the game's value
   (brief 1.0.1, 2.0.6, 7.0.1.2). The entity's tick count is its own, `Entity::age`
   (the ticks it has ticked, so a mob in a frozen chunk keeps its counters, as the
   game's do). What counts only when something runs (`TemptGoal.calmDown`, counted in
   `canUse` calls while the goal's flags are free: brief 2.0.3) is state, committed when
   it changes. `noActionTime` near a player, reset every tick, is kept as "the tick from
   which a player was within 32 blocks", derived from the players' positions (decision
   4) rather than written. The measurement of N7k says whether the rest keeps a standing
   animal free of writes.
4. **Players beyond the region.** A mob reads players: tempting (10 blocks), looking
   (6 to 8), begging, an owner to follow, a dolphin's boat, the nearest player within 32
   blocks for `noActionTime` and despawning, 128 blocks for despawning at once (brief
   1.0.2, 2.0.9, 2.0.10). Players are in no copy (N5's named difference). So:
   - **players in copies**: a commit carries, for every player of the region whose public
     face changed, that face (position, eye height, rotations, the items in both hands and
     worn, sneaking, the abilities, invisibility, the vehicle's key, alive, the game
     mode, `hasMovedHorizontallyRecently`), keyed by the chunk they stand in, and the
     store passes it on with a copied chunk's updates as it passes entity changes; a
     region reads players in its copies where it reads its own, never ticks or writes
     them. The plate's `PlayerInside` of N6 then reads them too and goes.
   - **the world's players**, for the reads beyond a copy (the 32 and 128 blocks): the
     store publishes on every region's stream, once a second, the position of every
     player of the world, as it will publish spawning's counts in N9 (ADR-0028,
     section 8). A region uses it only for distances to the nearest player beyond its
     land and copies; the second's lateness is named.
5. **Across a line, a mob reads its region's land and copies and nothing beyond.** A
   path search reads blocks as air beyond them, as the game reads land that is not
   loaded (brief 3.0.3); the 32 to 48 ranges of foxes, llamas, bees, allays, copper
   golems, happy ghasts and villagers plan shorter near a line (named). A mob takes a
   partner, a parent, a leader, a target or a flock only among the entities its region
   holds or copies; what it does to another region's entity is an entity effect
   (ADR-0028, section 7) with two new kinds: **breeding** ("in love with you": the
   partner's breed target and the birth's ages, brief 7.0.1.7) and **the tongue** (a set
   velocity, a hit, a removal if dead, brief 7.0.2.5); a frog has no prey in N7 (its
   prey are monsters). Where regions follow players, an animal that works at a line
   (breeds, follows a parent, targets or is targeted across it, or paths within two
   blocks of it) merges the two regions at once, as ADR-0028 section 7 says.
6. **A world clock, and the day.** Anger ends at an absolute game time, a camel's pose
   and a copper golem's weathering are game times, bees stay home at night, foxes sleep
   by day, a cat brings gifts in the morning (brief 5.0.2, 6.0, 7.0.1.3); Clustine's
   regions count their own ticks, and the edge sends no time. So each region's state
   carries the world's **game time** and **day time**, a tick on at every tick; a split's
   parts take their region's, a merge the later of the two, a new world 0; a commit
   names it, and a region restored takes it from its checkpoint and commits. Regions
   tick at 20 a second each, so their clocks stay within the ticks one lags behind
   (named: a cat across a line may see the morning a few ticks apart). The edge sends
   each player `set_time` from their region's clock as the game does (every 20 ticks and
   at join), with the day cycle running, as the game's default (`advance_time`).
   Weather stays clear until a later step (named; bees and foxes read rain and thunder
   as never).
7. **Randomness.** An entity draws from its stream (ADR-0028, section 4). What the game
   draws from the level's random on an entity's behalf (every behaviour's start, a
   golem's stroll, `finalizeSpawn`, the fish school, brief 2.0.8, 6.0, 7.0.1.4) comes from
   the drawing entity's stream, after its own draws of that tick, in the game's order;
   the gates' shuffles and the long jump's JDK shuffle too. Generation's draws are the
   game's exactly: the `WorldgenRandom` of `spawnOriginalMobs` and the region random of
   the spawn step (brief 8.0.2, 8.0.3). Every use of HotSpot's `Math.sin`, `cos`, `atan2`
   and `pow` (brief 2.0.7, 6.0, 7.0.1.5) is ported with `libm` (`StrictMath`) and named,
   ADR-0028's list of three amended. Where the game iterates a hash map by keys whose
   hash is fixed (activities at one priority, by their names: brief 7.1.1, finding 6),
   the port keeps the game's bucket order; where the order changes between runs (the
   parrot's imitation over entity types, brief 5.0.6; points of interest's ties, brief
   6.0) it takes a fixed order of its own, named.
8. **Paths are exact.** The path finder draws nothing (brief 3.0.1): it is ported
   exactly and judged node for node by running the jar's own `PathFinder` on fixed
   layouts inside the official server, where it needs a level and its main thread (a
   small Java agent, run by the lead; brief 12.3). Each state's base
   path type and `isPathfindable` for land, water and air come from the extract program
   (brief 3.0.10). The level's path-type cache is not kept: types are computed fresh,
   and a block change re-plans nearby paths at once in the region that changed it and
   when a copy's update brings it across a line (brief 3.0.4, 3.0.5; named where the
   game is stale).
9. **The villager before N10**: placed with its body, data, profession from the
   template, and a brain of the core and idle activities only (swim, look, stroll,
   panic, look at players, the baby's play), no points of interest, work, sleep,
   gossip, trading or golems' summoning (brief 7.14). The wandering trader is not
   spawned until N10 (`WanderingTraderSpawner`), but comes from its egg so.
10. **Monsters before N9**: from a spawn egg or a structure, a body (health, attributes,
    equipment drawn as the game's `finalizeSpawn` draws it, loot) with its goals that
    target nothing; burning in daylight, attacking and natural spawning are N9's (named).
    The witch of a swamp hut, the guardians of a monument and the illagers of a mansion
    are placed so.
11. **Experience** (brief 1.0.5, 1.7): the experience orb is ported, merging and moving
    to the nearest player as the game's; the player's experience (level, progress,
    total) is kept in their state and their place in the store, and sent
    (`set_experience`), though a creative player's bar is not shown by the client.
12. **The player's hit** (brief 1.0.4, 1.9): the region keeps each player's attack
    strength as the tick of its last reset, and the attack input carries what the
    server's copy of the player reads that the region does not have (sprinting, the
    fall distance, being on the ground as the client last said); crits, sweeps, both
    knockbacks and the damage window as the game's. N5's entities that a creative
    player breaks at once stay so.
13. **No item components yet** (as N6): a bucket of fish, an axolotl or a tadpole
    carries no data, and empties a fresh one of its kind (named); a renamed spawn egg's
    name is not read (no names on stacks); a spawn egg is one class read from data, its
    type extracted from its default `entity_data` (brief 1.0.15).
14. **Keys of what generation makes.** The pool's spawn step and the structures' template
    and code-placed mobs give keys from the chunk and the entity's index in it, in a
    range that no region's block ever gives (the key's top bit set), so that a chunk made
    again gives the same keys and the store takes them in exactly once; the chunk's
    entity record is written with the chunk (brief 8.0.19). A feature run's results carry
    entities per chunk of the three by three, as they carry blocks (brief 8.0.14). The
    three readings of the clock and calendar in generation (brief 8.0.18) are fixed: the
    camel's pose tick is 0, the difficulty easy's, Halloween never (named).
15. **Babies, ageing and despawning**: animals do not despawn (brief 1.0.3); fish,
    squid, dolphins, tadpoles and bats do, by the game's rules, from N7 (they come only
    from eggs until N9).

## 3. Parts, in order

Each part ends in a verified commit on `main`. The contracts (N7a) are mine and pushed
before anything is delegated; the store's and the edge's parts (players in copies, the
world's players, the clock, the keys of generation, the living packets) are mine; the
rest is delegated with files of its own, and N7l's tests are written from this plan and
the brief by someone who built none of it.

- **N7a, the contracts and the data.** `EntityData::Mob { body, mind, kind }`: the
  living body (health, the attributes' base values and modifiers, hurt and death
  times, the last hurt by whom and when, equipment and drop chances, persistence, the
  leash, left-handedness, `NoAI`, the experience reward), the mind (goal states and
  locks or the brain, the controls, the navigation), and the type's own state as one
  enum of the types; the codec in `clustine-format` (the store's format raised); the
  experience orb; inputs (`Attack` with what decision 12 needs); events and the edge's
  packets (`damage_event`, `entity_event`, `update_attributes`, `set_equipment`,
  `set_time`, `set_experience`, `mount_screen_open`); the extraction (default
  attributes, entity loot tables in 26.3's form, each spawn egg's type, each state's
  path type and `isPathfindable`, entity type tags), which I run.
- **N7b, the living body** (delegated): brief section 1: the living tick, hurt, death,
  loot, experience orbs, the player's hit, spawn eggs, name tags, leads on mobs,
  pushing, despawning, persistence; the edge's living packets (mine).
- **N7c, the AI's machinery** (delegated): brief section 2 (the goal selector, controls,
  sensing, targeting, random positions) and section 3 (navigation and the path finder,
  exact); the jar's `PathFinder` harness (mine).
- **N7d, players beyond the region and the clock** (mine): decisions 4 and 6: players in
  commits and copies, the world's players on every stream, the region's clock, merges and
  splits of it, `set_time` at the edge.
- **N7e, farm animals** (delegated, after N7c): brief section 4.
- **N7f, tamed and ridden animals** (delegated, after N7c): brief section 5, with the
  horse's menu on N6's menus.
- **N7g, wild and water animals** (delegated, after N7c): brief section 6, with bee
  hives and turtle eggs.
- **N7h, brains and their animals** (delegated, after N7c): brief section 7.
- **N7i, golems, villagers and monsters' bodies** (delegated, after N7c and N7h):
  decisions 9 and 10, brief 8.15 and 7.14.
- **N7j, made with the chunk** (the pool's step delegated, the keys and the store mine):
  brief section 8 and decision 14; the jigsaw's missing draws mended.
- **N7k, measured and compared**: a pen of 100 animals wandering and 20 sheep following
  a player with wheat: the log's growth and the tick's length, on memory and on the real
  disk; the comparisons with the official server: the animals of chunks of two seeds,
  exactly (type, position, yaw, variant, colour, baby, horse stats); a summoned mob's
  packets (spawn, data, attributes, equipment, a hit, a death); paths node for node by
  the harness.
- **N7l, tests from the specification.**

## 4. Named differences expected

- A path near a line reads nothing beyond the copies; the long ranges plan shorter there;
  the level's path-type cache is not kept (decisions 5 and 8).
- The nearest player beyond the copies is known a second late (decision 4).
- The clocks of two regions differ by the ticks one lags; weather is always clear
  (decision 6).
- A fixed order where the game's iteration changes between runs (decision 7).
- A chunk taken up again after nobody held it keeps its mobs' goals and brains, where the
  game would start them again (decision 2).
- Buckets of fish carry no fish; renamed eggs name nothing (decision 13).
- Generation's camel pose, difficulty and Halloween fixed (decision 14).
- Villagers do no work, sleep, gossip or trade; monsters do not attack or burn
  (decisions 9 and 10).
- Water creatures, bats and axolotls come only from eggs until N9.

## 5. Open questions for the review

1. Is the AI's state in the record (decision 2) affordable, and is never resetting it,
   even where the game would reload a chunk, right?
2. Are players in copies (decision 4) sound against hand-overs, merges and splits: a
   player's face in a copy while they cross, two regions' faces of one player?
3. Is a clock per region, merged by the later (decision 6), good enough, or does the
   world need one clock the store keeps?
4. Do the two new entity effects (decision 5) and the merge for animals at a line cover
   what animals do across one?
5. Is N7 one step, or should the brains (N7h) and the wild animals (N7g) be a step of
   their own after it?

## 6. Questions for the owner (the recommendations are taken; each can be overruled)

1. **The day passes** (decision 6): night falls from N7, with no monsters until N9.
   Recommended, as bees, foxes, cats and turtles need the day.
2. **Weather** stays clear until a later step (with N8's light, as rain darkens the
   sky), or comes now?
3. **Monsters from eggs before N9** are bodies that stroll and target nothing
   (decision 10), rather than eggs that do nothing.

## 7. The first two reviews, and what they changed

Two independent reviews of the draft (2026-10-19): one against Clustine's code and
records (R1, 31 findings), one against the game and the brief (R2, 35 findings). This
section overrides what it amends above. Brief 7.0.2.2 was wrong about
`Mob.finalizeSpawn`'s random (R2.22): the accessor's, as 8.0.2 says; mended in the brief.

### 7.1 The step is two steps, cut at the machinery (R1.31, R2.25; open question 5)

Cutting by subject would leave frogs, goats, foxes, wolves and bees met with their chunks
but standing still. So:

- **N7** builds the body, the AI's machinery, players beyond the region, the world clock
  and sleep, and the farm animals from spawn eggs, and measures the AI's record before
  anything else is built on its encoding (decision 2 is then answered by numbers). Its
  parts: N7a the contracts, N7b the living body, N7c the machinery, N7d players and the
  clock, N7e farm animals, N7f the path harness and the measurement, N7g tests from the
  specification.
- **N11** (the phase's next free number, run straight after N7, before N8) builds the rest:
  N11a tamed and ridden, N11b wild and water, N11c brains and their animals, N11d golems,
  villagers and monsters' bodies, N11e made with the chunk (only once every creature
  behaves), N11f the light-free natural spawning if the owner takes question 7.12.6, N11g
  measured and compared, N11h tests from the specification. What to try is written after
  N11.

### 7.2 The AI's state and its counters (decisions 2 and 3)

1. **Durable, in fields of its own** (R1.3). The mind is not one `Data(whole)`: the
   record's entity fields gain the mind's parts apart: the goals' running set and locks,
   each goal's fields, each control, the navigation's path (written whole when a path is
   made), its next index and its timers apart, each brain memory apart. A path is kept as
   its start and its steps (R2.32). A merge commits only `Age` of an absorbed mob, not its
   data, once every counter is a stamp (R1.3c).
2. **Copies carry no mind.** The store leaves the mind's fields out of copies' bases and
   updates: a copy is never ticked, and what other mobs read of a mob across a line is its
   body and type state (love, age, a baby, a leash), never its goals. Reads of another
   mob's mind across a line (a villager reading another's path at a door) see none, named
   (N10).
3. **Sensors' lists are not durable** (R1.3d): memories a sensor computes from the world
   (the nearest and visible entities, players, items) are left out of the record and made
   again at a take-in, the sensor running at its next due tick.
4. **The entity's clock is its age, and its age is written with every mob change**
   (R1.1): every `Set` of a mob carries `Age` (four bytes), so a stamp is never ahead of
   the restored age; a mob that committed nothing since its last record has its counters
   as of that record after a crash, as N2 names for items.
5. **Counters that others read commit when they cross** (R1.2): love ending, growing up,
   an age lock's end, a breeding cooldown's end, a camel's pose change, a sheep's wool
   regrowing: each writes its flag or value in the tick it changes, so a copy sees it
   within a tick. N7a lists them per type with the type's state.
6. **`noActionTime`** (R1.4, R2.11) is the tick of its last reset, written at a hurt and
   when the nearest player leaves 32 blocks; while a player is within 32 it reads 0. A
   region with no publication of the world's players yet reads every player distance as
   near (R2.16).
7. **The game's own reload is matched where nobody can see it** (R2.10, overriding the
   draft's last change to decision 2): a chunk taken up by a load from the store when no
   region held it is loaded as the game loads a chunk: goals start empty, a brain is made
   from its saved memories, unsaved fields reset (the fox's target goals set, the frog's
   locked age cured), `tickCount` from 0. Everywhere else (crossings, merges, splits,
   lends, returns, restores) nothing is reset. R1.5's stale path is then moot: what was
   held keeps its copies' updates, what was not held starts again.
8. **The entity's stream is keyed by its age** (R1.6, R2.12): `Random::of_entity(seed,
   key, age)`, monotonic for each entity across lines and merges, where the region's tick
   is not; draws made on an entity outside its own tick (a hit, a breeding effect) take a
   sub-stream of its own. Items move to it with mobs (their draws are distributions only).
   A draw the game makes from the level's random on an entity's behalf is taken at its
   place in the entity's stream, not after its own (R2.12). Draws the server makes for
   particles it never sends are made and discarded, so later indices are the game's.

### 7.3 Players beyond the region (decision 4)

1. **Faces live in the store's player records, not in chunks** (R1.7, R1.8). The store
   keeps one `PlayerRecord` per player, ordered by (stay key, `hops`), written from each
   commit's stay note in every tick a player's input is applied and for every player on
   the first tick after a restore, merge or split. The face (position, eye height,
   rotations, both hands and what is worn, sneaking, the abilities, invisibility, the
   vehicle's key, alive, the game mode, sleeping, the last hurt mob and its time, the last
   hurt by and its time, `hasMovedHorizontallyRecently`: R2.17) is added to the place.
   The store files the latest face by the chunk of the player's position and puts it on
   the stream of every region that copies that chunk, dropping it from the old chunk when
   the player moves or leaves; a copy's base takes its chunk's faces from that table as of
   its sequence. One face per player; a region ignores a face of a player it holds.
2. **Through a hand-over the face stays** (R2.2): the record keeps the player's last face
   until the receiver's commit replaces it, so a tempted sheep does not lose the player
   for the ticks they are in neither region, and a dog does not sit.
3. **The world's players are the same table**, published on every region's stream once a
   second (key, position, game mode, sleeping), a leave removing its player in the same
   group. It answers the 32 and 128 blocks, owners beyond the copies (R1.9: a pet resolves
   its owner from it, a second late; its teleport to an owner in another region's land is
   an arrival there), and sleep (7.4.4).
4. **`PlayerInside` stays** until N6's plate tests at a line pass with faces (R1.11).
5. **A leash held by a player in a copy holds** (R2.18): the pull is the leashee's
   region's, from the face; N5's snap at a pinned line goes. Carts and boats still pass
   through a player beyond a pinned line (named), since a push on a player is the player's
   region's.
6. **What an animal gives a player across a line** (R1.10): a new effect on a player's
   region, `Given { player, hand, stack }` (milk, stew, a filled bucket), done by the
   player's region as the game's `createFilledResult`. A parrot on a shoulder is part of
   the player's state, place and transfer (N7a), and lands only on a player its region
   holds (named).

### 7.4 One world clock, the 26.3 clocks, and sleep (decision 6; R1.12–15, R2.4, R2.6, R2.13)

1. **One clock that only moves forward.** Each region's state keeps the world's clock
   (its game time and each 26.3 clock's state, `ServerClockManager`'s, now dumped), a
   counter in `RegionState` and never a tick number (ADR-0024 section 3). The store
   publishes, with the world's players, the highest committed game time once a second;
   each region takes the later of its own and the published at every tick, and the change
   is committed; a merge takes the later of the two, a split's parts their region's, a
   hello the store's. Drift is then about a second plus a region's lag; a region restored,
   slowed or given up jumps forward, never back. Every commit carries the clock, and
   checkpoints and stops write it (R1.14).
2. **Game-time stamps** in entities (anger's end, a camel's pose, a behaviour's end, a
   path's last recompute) are absolute in the world clock; with the drift bounded a camel
   crossing a line may read its pose up to a second off, named (R1.12).
3. **The client's time**: the edge sends the 26.3 `set_time` (game time and the clocks'
   states, read from `ClientboundSetTimePacket` before N7a fixes the event: R1.15, R2.6)
   at join, as the server does, and at once when a player's region changes. Environment
   attributes that clocks drive through timelines (`bees_stay_in_hive`,
   `cat_waking_up_gift_chance`, `sky_light_level`, `turtle_egg_hatch_chance`, the
   villagers' schedule, `eyeblossom_open`) are read from the timelines, one implementation
   for all of them and for N8 and N9.
4. **Sleep comes with the night** (R2.4): beds are used, a player sleeps, and the night is
   skipped when the game's `players_sleeping_percentage` of the world's players sleep
   (from the world's players); the clock then moves to the morning in that region and the
   others take it at the next publication. N7d, the lead's (a player's state).
5. N6's sculk vibrations and hoppers, which read the region's tick as game time, move to
   the clock.

### 7.5 Across a line (decision 5; R1.16–19, R2.14, R2.15, R2.19)

1. **What is remembered is kept when it cannot be seen** (R2.14): a path search or a scan
   reads land beyond the land and copies as the game reads an unloaded chunk; the check of
   a remembered thing (a bee's hive, an allay's note block, a turtle's home, a pet's owner
   by key) beyond them answers "unknown", and the memory is kept.
2. **Breeding at a line: the lower key breeds** (R1.16): an animal whose partner it sees
   only in a copy breeds only if its key is the lower, as the game's order in one region
   would have it; the effect on the partner resets its love and sets its age. No twins.
3. **The effects** (R1.17, R2.15): breeding (above); an alert (`HurtByTargetGoal`'s, a
   target set on another mob); the copper golem's flower; a bee entering a hive whose
   block entity is beyond the line (its `Gone` here and the hive's occupant there in one
   commit's effect); the hit's effect carries whether a player struck it, so a mob killed
   across a line drops `killed_by_player` loot. Named instead: a fox's or panda's pick-up
   of an item seen in a copy (items are picked up in their own region, ADR-0028 section
   7), a copper golem's chests beyond its region's land (it uses what its region holds),
   mob effects (none until N9: the dolphin's grace, a bee's poison, a pufferfish's), a
   copy's entity counted as ticking for `isPushable` (R1.17). The tongue's effect goes to
   N9 with its prey (R1.18, R2.15).
4. **Merging narrowed** (R2.19, R1.19): where regions follow players, two merge for an
   interaction across their line (a breeding, a parent followed, a target, an owner's
   teleport, a leash), not for a stroll near it, which crosses as an arrival. Splits keep
   clear of such reports for 600 ticks, as for arrivals. N11g measures how often merges are
   made and splits refused with generated animals and players moving apart.

### 7.6 Keys and records of what generation makes (decision 14; R1.20–23, R2.20)

1. **A chunk's generated entities are taken once.** The store's note of a generated chunk
   writes its entity record in the same round as its blocks (R1.21; tested on the file
   store, not only the memory store); a chunk once handed out never takes the generator's
   entities again: a remake (`remakes`) takes blocks only and keeps the stored entities,
   an empty list included, and a flag in the record says the entities were taken, which
   survives the record's emptying (R1.20, R2.20). An arrival into a chunk not yet made
   merges with the generator's entities by key.
2. **The key layout, fixed in N7a** (R1.22): the top bit set, then a bijective mix of
   (dimension in 2 bits, chunk x and z in 22 signed bits each, the entity's index in the
   chunk, whether a structure made it), so that the low 32 bits (`game_id`) are scattered
   and no two mobs of a chunk run their selectors in step; region blocks are refused at
   2^31; no stay is ever a generated key; nothing keys by `game_id` (checked in N7a).
3. A generated record's counters are relative to its first take (R1.23), as N2 rebases.

### 7.7 The player (decisions 11 and 12; R1.24, R1.25, R2.9)

1. **The player's new state is the lead's**: the attack strength, the experience, the
   shoulders, sleep, the faces' fields, in the state, the place and the transfer, fixed in
   N7a and carried by me; N11a may not change the crossing paths of `riders.rs` and
   `riding.rs` (R1.24).
2. **The attack strength is relative**, ticks since its reset kept in the transfer and the
   place and rebased at an arrival and a merge, not a region's tick (R1.25). It is reset by
   an attack, by a punch (the `Punch` input of `f64decc`), by a successful use's swing and
   by a change of the held item (R2.9).
3. **The hit's data**: the extraction adds each item's default `attribute_modifiers`,
   `weapon`, `attack_range`, `minimum_attack_charge`, `damage_type` and `equippable`
   (R2.9, R2.27); the fall distance is the region's own from the moves.
4. **A player who leaves riding a horse** (R1.29): the game takes the vehicle with a lone
   rider and puts it back at the next login; in N11a the ridden mob goes into the player's
   place (ADR-0020's note), the lead's.

### 7.8 The contracts (N7a), the edge and the data (R1.26–28, R2.23, R2.24, R2.27, R2.28)

1. **The mob's tick is one skeleton**, with the hooks in the game's order (brief 2.0.12:
   `baseTick`, then `aiStep`'s sensing, target and goal selectors, navigation,
   `customServerAiStep`, the controls, then travel, block effects, body rotation, pushing),
   so that N7b and N7c own separate files (R2.23); the animal parts come after both.
2. **Each type's state is its own versioned record**, dispatched by type (R2.24): the
   format of a mob's record is raised once, in N7a, to `body`, `mind` and the type's own
   bytes with a version, which each type's module reads and writes; a part adds a module,
   not an enum variant. The store's format and `FEATURES_VERSION` are raised, so a world
   made before N7 is refused and made again.
3. **The edge** (R1.27, R2.28): every living entity's moves as the game's stepped path
   (`SteppedInterpolationTracker`, dumped; armour stands too), `rotate_head` on update
   ticks only, `update_attributes` following the instances the record says exist,
   `animate`'s crits, a mob's own `swing_animation`, `hurt_animation` to a player about
   itself, `set_entity_link` for mobs, the ambient, hurt, death and step sounds; the
   attribute ids the registry's.
4. **Wire ids are reused** (R1.28), with ADR-0028's quarantine until every connection has
   answered a ping sent after the removal: the lead's, with the edge, in N7a.
5. **The extraction** adds (R2.27): the items' default components (7.7.3), `isValidSpawn`
   per state and creature type, the spawn placements, the types' `immuneTo` tags, the
   fluid and damage-type tags of brief 2.0.16, beside the draft's list.

### 7.9 What the player meets, corrected (R2.1, R2.3, R2.5, R2.7, R2.8, R2.33–35)

1. **Name tags do not work** (R2.1): a creative tab's tag has no name, and there is no
   anvil and no component on a stack; struck from section 1 and named. Question 7.12.4 asks
   the owner for a step of item components.
2. **Section 1's list** (R2.7): no ocelots are made with a jungle's chunks (they are
   `monster` there, natural spawning's); cats come only from villages and swamp huts;
   bastions are in the Nether, not generated; the mooshroom is on mushroom fields.
3. **Despawning** (R2.8): every mob despawns by `Mob.checkDespawn` and its type's
   `removeWhenFarAway`, from N7, with the overrides of brief 1.0.3 (an untamed cat or
   untrusting ocelot after 2400 ticks, an axolotl not from a bucket, the nautilus, the
   camel husk, the zombie horse, the chicken jockey) and every monster body from an egg.
4. **Villagers** (R2.3): in the game they take work and beds within a minute, which needs
   points of interest (N10). Question 7.12.5 asks the owner; recommended for N11d: the
   villagers' schedule read from its timeline, so that they rest (stand still) at night
   rather than wander, professions, beds and trades in N10.
5. **Water** (R2.5): water creatures' natural spawning reads water and the sea level, not
   light. Question 7.12.6 asks the owner; recommended: natural spawning for `water_creature`,
   `water_ambient` (and ADR-0028 section 8's counts and caps) as N11f, ahead of monsters.
6. **Monster bodies' goals** (R2.33): float, stroll, look at players and look around; the
   rest (a zombie stomping turtle eggs, a creeper avoiding cats, a skeleton's sun goals)
   with N9, named.
7. **The "visible" range in a static** (R2.31) is per region, named; the bee's hive search
   is the game's scan of its 5 by 5 chunks; the dolphin's treasure reads the structures'
   starts, which exist (R2.34); a spawn egg on a spawner sets its entity, though it spawns
   nothing until N9 (R1.30, R2.35); the horse's menu closes at a line, as N6's menus do
   (R2.26, named); copper golems count as a chest's openers and allays hear note blocks
   through listeners that move with entities, both in N11c (R2.26).

### 7.10 The comparisons (R2.21, R2.29)

- **Paths**: a Java main on the jar's class path starts the server on a thread and runs the
  cases through its `execute`, on the main thread with the chunks at `FULL`; the mob is
  given `OnGround`, and both `PathFinder.findPath` and `navigation.createPath` are run.
- **Animals made with chunks**: the official server's ticks are frozen before any compared
  chunk is generated; type, position, yaw, variants, colours, babies, horse stats, the
  follow range's modifier and the left hand are compared exactly; the constructor's draws
  and the UUID are not. The structures' entities of a seed with a village, an outpost and a
  swamp hut too, and the region random's state after a village piece (brief 8.17).
- **A summoned mob**, with data and a rotation, so that its spawn, data, attributes and
  equipment are exact; a hit's damage event, health and knockback exact where the bot
  controls the attack strength; sounds' pitches, loot and the panic by bounds; breeding's
  packets; a tamed wolf's flags; `set_time`.

### 7.11 Section 3 as it now stands

- **N7a, contracts and data** (mine): 7.8, 7.6.2, 7.7.1, the faces and the world's players'
  publication's shape, the clock in `RegionState`, the effects of 7.5.3, the wire ids'
  reuse, the extraction.
- **N7b, the living body** (delegated): brief section 1 on the skeleton of 7.8.1; the
  edge's living packets (mine).
- **N7c, the machinery** (delegated): brief sections 2 and 3, the paths exact.
- **N7d, players beyond the region, the clock and sleep** (mine): 7.3, 7.4.
- **N7e, farm animals** (delegated, after N7b and N7c): brief section 4, from eggs.
- **N7f, the path harness and the measurement** (mine): a pen of 100 animals, 20 sheep
  following wheat, a flight over land with eggs' animals, a merge of two regions with
  animals, a crash's restore of a region with 500 mobs: the log, the copies' updates and
  the tick's length, on memory and on the real disk; the encoding of 7.2.1 adjusted from it.
- **N7g, tests from the specification.**
- Then **N11** as 7.1 says, its parts delegated as section 3 had them, N11e's store side
  and N11a's riding mine.

### 7.12 Questions for the owner, as they now stand (recommendations taken; each can be overruled)

1. **The day passes**, with sleep in beds to skip the night (7.4). Recommended.
2. **Weather** stays clear until a later step (with N8's light, as rain darkens the sky).
   Recommended.
3. **Monsters from eggs before N9** are bodies that stroll and target nothing (decision
   10). Recommended.
4. **Item components** (name tags and the anvil's rename, a bucket's fish, a shulker box's
   contents when broken, a renamed egg): a step of their own after N11. Recommended.
5. **Villagers before N10** rest at night and stroll by day, without work, beds or trades
   (7.9.4). Recommended; the alternative is points of interest and professions in N11.
6. **Natural spawning of water creatures** (and its counts and caps) in N11, ahead of
   N9's monsters (7.9.5). Recommended.

## 8. The third review (of section 7), and what it changed

An independent review of section 7 (R3, 28 findings), against the code at `f64decc` and the
26.3 bytecode of the clocks, timelines, `ClientboundSetTimePacket`, `SleepStatus` and
`BedBlock` (dumped after the brief; `MinecraftServer` read from the jar without a JVM). This
section overrides section 7 where it amends it.

### 8.1 The halves keep the agreed order (R3.28)

The owner agreed "N1 to N7, then N8, then N9 and N10". Section 7.1's second step is N7's
second half, not a step after the phase's last: it is renamed **N7, second half**, its
parts N7h to N7o (7.1's N11a to N11h in order: N7h tamed and ridden, N7i wild and water,
N7j brains, N7k golems, villagers and monsters' bodies, N7l made with the chunk, N7m the
light-free natural spawning if taken, N7n measured and compared, N7o tests from the
specification), and wherever section 7 says N11 it means this. N8 follows the second half.
**Water creatures' natural spawning stays with N9** as agreed (7.12.6 withdrawn: it is
most of N9's counting machinery, with its churn in the log, R3.27); named until then. What
to try is written after each half (R3.26).

### 8.2 Loads: fresh, continued, and the entity's two clocks (R3.1–6)

1. **`StoreReply::Loaded` carries `fresh`**, set by the store from ADR-0025 section 5's use:
   a load is fresh when its chunk had no use when the commit thread passed it on. After a
   store restart every held chunk is in use until `Dropped`, so a load errs towards
   "continued", the safe side (named). `take_up_entities` takes three cases: **fresh** (the
   game's load: 7.2.7), **continued by a new holder** (a raise of `hops`, as an arrival),
   and **continued by the same lineage** (a merge's or split's warm chunk, a restore),
   which raises nothing and writes only `Age`, as 7.2.1 wants. The comment at
   `region/entities.rs:129-138` changes with it. The lead's, in N7a.
2. **Two counters** (R3.2): `lived`, the ticks an entity has ticked, never lowered or
   reset by any rule, keys the entity's stream and every stamp; the game's own counters
   (`tickCount`, an item's age) are derived from it (`tickCount = lived − lived at the last
   fresh load`; an item's age kept as its own value, which a merge lowers). A fresh load
   turns each stamp back into the game's saved form where the game saves it and drops the
   rest.
3. **Each type's module has a fresh-load form** (R3.3): what the game's codec saves, and
   the memories whose type has a codec (the extraction lists them).
4. **The body and the mind** (R3.4): N7a lists both. `target`, `isAggressive`,
   `lastHurtByMob` with its time and the leash are the body's; the mind is the goals' and
   the brain's state only, and only it is left out of copies.
5. **Sensors run in the take-in tick** (R3.5), once, before the brain, from the taker's land,
   copies and faces, drawing nothing; the next due tick is unchanged.
6. **The sub-stream** for draws on an entity outside its own tick (R3.6) is keyed by (key,
   lived, the world's game time) with a count within the tick, as `entity_draws` does.

### 8.3 Faces: through a hand-over, at a leave, in order (R3.7–12)

1. **The sender keeps a departed face** for the stay (as N5 keeps a seat) until a face of
   that stay with `hops` at least the departure's comes on its stream or the player is
   taken back; **the store also streams a face to the holder of the face's chunk** when
   that holder is not the player's region (which covers a riding player in land their
   region does not hold). The test R2.2 proposed is in N7g: twenty sheep tempted across a
   pinned line never stop.
2. **`StayNote::Gone { player, entity, hops }`** is written for every removal (a leave, a
   replacement, a dead stay, an edge gone after 600 ticks, a `Discard`); the store drops
   the face and the world list's entry at a `Gone` of the living stay and at an `Entering`
   that raises the floor; the players' file keeps the place and marks the record offline.
   ADR-0020's table gains the row.
3. **Faces carry the sequence of the commit that wrote them**; a region keeps one face per
   player and takes one only if its sequence is higher, base faces included (R3.9).
4. **Faces follow copies**: where copies are withdrawn when two regions uncouple (a slow
   region given up), the faces go with them (named, operational and rare; R3.10).
5. **A pet's teleport to an owner beyond the copies** is an entity effect to the owner's
   region (R3.11), which runs `tryToTeleportToOwner` with its own blocks and takes the pet
   in as an arrival if a try succeeds; within the copies it is the pet's region's.
6. **The face's hurt timestamps are per-stay counters** carried in the transfer and the
   place (R3.12), as the attack strength is (7.7.2).

### 8.4 The clock and sleep, as 26.3 has them (R3.13–16)

1. **Three ratchets** (R3.14): the game time, and each `world_clock`'s
   `(total_ticks, partial_tick)` (`overworld`, `the_end`); rate and paused are settings, not
   ratcheted. Each is published with the world's players and each taken by its own later.
   A sleep moves the overworld clock (`moveToTimeMarker(…, WAKE_UP_FROM_SLEEP)`), not the
   game time. A ratchet never turns time back; a later `/time set` earlier needs a
   world-wide barrier (named).
2. **The store keeps the world's clock durably** (R3.15), raised by every taken commit and
   written with checkpoints, in the regions' table's file; a hello and the publication read
   it.
3. **What the client is sent** (R3.13): the game time alone every 20 of the region's ticks
   (`forceGameTimeSynchronization`); the full clock states at join, at a change of region,
   and whenever the region's clock moved other than by its tick (a ratchet's jump, a
   sleep's skip, a restore), which the region reports in its delta. N7n compares them.
4. **Sleep as the game's** (R3.16): the night is skipped when enough of the level's
   players sleep and enough sleep deeply (`sleepCounter >= 100`), spectators not counted;
   the face and the world list carry the sleep counter (in lived ticks) and the game mode;
   every region evaluates the rule over the world list with its own players in place of
   their entries. The skipping region wakes its sleepers; others wake by the bed rule once
   their clock reaches the morning, a second late (named). N7d's work: the bed's
   `occupied`, the sleeping pose and `sleeping_pos` shown to others, the leave-bed input,
   the respawn point a bed sets (state, place, transfer), `BedRule`'s `when_dark` from the
   clock, and the action bar's `sleep.players_sleeping` and `sleep.skipping_night` to every
   player of the level, across regions, from the world list.

### 8.5 Across a line (R3.17, R3.18)

1. **Breeding across a line is done by the partner's region** (R3.17): an animal that sees
   its partner only in a copy sends "breed with me"; the partner's region breeds only if
   the partner is still in love and has not bred in that tick, and the child, the orb and
   the sender's reset follow as effects. No twins. 7.5.2's lower-key rule is withdrawn.
2. **Two more effects** (R3.18): a player's use that converts its target across a line (a
   mooshroom sheared into a cow) is the holder's, with the new key from the holder's
   block; a spawn egg used on a mob across the line makes its baby in the holder's region.
3. ADR-0028 sections 4 (the stream's key) and 7 (the merge trigger, the effects) are
   amended when N7a lands.

### 8.6 What generation makes (R3.19–21)

1. **A chunk with a stored blocks file never takes the generator's entities** (R3.19): the
   note writes both files in one round, so the blocks file is the flag; `remakes` drops its
   `entities()` condition, takes new blocks and keeps the stored entities, an empty list
   included. A remade chunk gets no template entities (named: a village a newer generator
   places there has no villagers).
2. **The second half refuses worlds made under the first** (R3.20): its generator makes
   entities, so the store's format and `FEATURES_VERSION` are raised again at N7l, as at
   every change of what a chunk is made with (named in what to try).
3. **The key's mix** (R3.21) stays; 7.6.2's claim about selectors in step is withdrawn
   (two of any three mobs share a phase).

### 8.7 The contracts (R3.22–25)

1. **A mob's record** (R3.22): `EntityData::Mob { body, mind, kind }` where `mind` is a list
   of (goal or memory id, version, bytes) **outside** the type's bytes, so that the store
   leaves it out of copies by its tag without knowing types, and `kind` is (version,
   bytes); postcard carries them as bytes and the format writes them length-prefixed. The
   region keeps the typed state beside the chunk, decoded at take-in and encoded on change.
   Each module reads every older version of its bytes (a type of the second half reads the
   empty version-0 record an egg of the first half made) and has its fresh-load form.
2. **The edge's part of each type** (R3.22d): each module gives a neutral `Shown` (the
   values the type shows), and the lead maps it to the type's synched data at the edge in
   the same part.
3. **N7a writes, as types and traits with a test each, before N7b and N7c are briefed**
   (R3.23): the shared body (speed, the inputs `xxa`, `yya`, `zza`, jumping, the head's and
   body's yaw, target, the last hurt by with its time, `noActionTime`'s stamp), the read
   view (blocks from land and copies, air beyond; entities in a box; players from holdings,
   faces and the world list; "unknown" for remembered things), the borrow pattern (the mob
   `&mut`, the rest `&`), the draw API (the stream at the game's place, the sub-stream), the
   outputs (sounds, entity events, packets in neutral form) and the attributes.
4. **The lead's parts** (R3.24): fresh and continued loads, the mind out of copies, the
   merge's `Age`, sensors at a take-in, mobs' arrivals with their mind and `lived` (N7a and
   N7d). **Pig riding** moves to N7h with the other ridden mobs and the logout of 7.7.4
   (named for the first half: a saddled pig is not ridden yet).
5. **The brain's encoding** is fixed in N7j from its own measurement (R3.25); N7a fixes the
   goals' mind only, and the versioned bytes let N7j choose without refusing worlds.

### 8.8 What a player meets in the first half (R3.26)

- **Thrown eggs** hatch no chicks (projectiles are N9's): named.
- **Spawn eggs of types not yet built** make nothing in the first half (named).
- **Light** before N8: what `getWalkTargetValue` and the spawn rules read is the stored
  light of the chunk, as generation lit it and as it stands (Clustine keeps light for
  clients), darkened by the clock as the game darkens sky light; named where blocks
  changed it, until N8.
- **What to try** is written after the first half: farm animals from eggs, breeding,
  babies, shearing, milking, tempting across a pinned line, a hit and a death with its
  orbs, the day and the night, sleep.

### 8.9 The owner's questions, as they now stand

1. **The day passes, with sleep** (8.4): beds, the pose, the respawn point, the messages
   across regions; and time can never be turned back by the ratchet. Recommended.
2. **Weather** stays clear until a later step. Recommended.
3. **Monsters from eggs before N9** are bodies that stroll and target nothing, from the
   second half; in the first half their eggs do nothing. Recommended.
4. **Item components** a step of their own after N7 (name tags, a bucket's fish, a shulker
   box's contents, a renamed egg). Recommended.
5. **Villagers before N10** wander by day and follow their schedule's rest at night, as
   the game's rest package does without points of interest (it still wanders a little:
   R3.27), without work, beds or trades. Recommended.
6. **Water creatures** come with N9's spawning, as the agreed order has it (8.1); oceans are
   empty until then but for eggs. Recommended.
7. **Worlds are refused twice**: at the first half (the mob's record) and at the second
   (the generator makes animals). Recommended, as before every change of what a chunk
   holds.

## 9. The fourth review (of section 8 and the whole), and what it changed

An independent review of section 8 and of the plan taken together (R4, 27 findings),
against the code at `f64decc` and the dumps. This section overrides what it amends.

### 9.1 Loads (R4.1–8)

1. **A "taken fresh" mark** (R4.1): use begins at a claim's grant, before the load, so use
   alone never says fresh. The commit thread keeps per chunk a mark set when its use begins
   from none (a grant of a chunk nobody held, or a load of a held chunk not in use), cleared
   when use moves between regions (merge, split, lend), read into `Job::Load` when the load
   is passed on and cleared by it; `Loaded.fresh` is the mark. A chunk whose use ended while
   another region copied it counts as continued (R4.5: in the game it stayed loaded in that
   player's view). Written into ADR-0025 section 5 beside use.
2. **Two axes, not one** (R4.2, R4.6): *reset or not* (fresh: the game's load; else
   continued) and *raise or not* (`hops` is raised at every take-in except a merge's, whose
   survivor the edge's `stands_for` covers; a fresh load raises too). The raise is a `Set`
   of `Hops` and `Lived` (a new `EntityField::Hops`), not a `Whole`, so the saving of 7.2.1
   holds; ADR-0028 section 6 is amended so.
3. **How a chunk was taken** (R4.3) reaches the tick with it: `chunks_loaded` carries
   `Took::{Fresh, Moved, Merged}`, set by the runner (`Fresh` from `Loaded.fresh`; `Merged`
   for warm chunks after a merge and loads the merge's answer listed; `Moved` otherwise: a
   split's warm chunks, a lend's borrower, loads after a restore or a move).
4. **A lend's end is continued** (R4.4): at the release the use moves to the pinned
   region, as `lent` moves it to the borrower. Tested: a lend that ends keeps a tempted
   sheep tempted.
5. **`lived` and the item's age** (R4.7, R4.8): `Entity::age` becomes `lived`
   (`EntityField::Lived`); `Entity` gains `fresh_at` (lived at the last fresh load), from
   which `tickCount` is derived; `EntityData::Item` gains its own `age`, which a merge
   lowers; every `first_tick` test (items, stands, boats) reads `tickCount == 0`; carts,
   boats, stands, falling blocks, TNT and dispensed entities move their streams to `lived`
   with items; spawn-time draws become sub-stream draws; the own stream and the sub-stream
   have distinct salts. `lived` is never lowered except by a restore after a crash, which
   re-lives the ticks since the entity's last record. The lead's, in N7a.

### 9.2 Faces (R4.9–13)

1. **The departed face ends** (R4.9) at the first of: a mark of the sender's stream at or
   past its departure commit's sequence, a removal of that face, a `Gone` of the stay, or
   100 ticks. It is kept in memory, not in the state (a restore forgets it, a second's
   seam after a crash, named).
2. **Removals are streamed** (R4.10): the store streams a face's removal `(player,
   sequence)` to every region it streamed the face to (the old chunk's copiers and holder)
   when the face leaves the chunk, at a `Gone` and at a raised floor; a region applies it
   as a tombstone, dropping a face of lower sequence. A region that begins to hold a chunk
   (a grant, a merge, a split, a lend) is sent that chunk's faces of other regions' players
   as a base as of the group. A face's sequence is (group sequence, index in the group).
3. **`Discard` carries the player and the transfer's `hops`** (R4.11), the edge's change,
   the lead's; ADR-0020 section 14 lists it.
4. **`Gone`'s rule** (R4.12): the store keeps with the face the region that wrote the
   stay's latest taken note; a `Gone` counts only from that region with `hops` at least the
   record's; a later `Has` of the living stay brings the player back; when a region's first
   tick names every stay (after a restore, a merge or a split), the store marks offline the
   players last written by that region (or the absorbed one) that it does not name. Faces,
   `Gone` and the offline mark are in `players.rs`'s undo and on streams only once durable.
5. **Their cost** is measured in N7f (R4.13): the faces' share of the stream and the log at
   20 sheep following a player.

### 9.3 The clock and sleep (R4.14, R4.15)

1. In a tick where the region's clock moved other than by its tick, the bed rule's wake
   runs for its sleepers before the sleep check, so a ratchet's jump never skips a second
   night (R4.14).
2. Named: the world's day runs at the fastest region's pace (a pair at 22.4 ticks a second
   makes a day about 11 % short). Players carry the game's `sleepCounter`; the world list's
   is up to a second low, the conservative side. The commit carries the clock as a field of
   its own, outside the opaque state (R4.15).

### 9.4 Breeding across a line (R4.16–18)

1. **Only the higher key asks across a line**; the lower key's `BreedGoal` reaches its count
   and does nothing, and its region answers the request. **An asker is marked "asked"**
   until answered or for 40 ticks: no free partner, chooses none, does not breed at home.
   The partner's region answers *refused* (clears the mark) or *accepted* (R4.16).
2. **The partner's region breeds as the game's `this`**: the child, the orb and the hearts
   at the partner, from its stream; only the asker's `setAge(6000)` and love's reset cross,
   in *accepted*. Named: the child appears at the partner (R4.17).
3. The protocol, its effects and the asked mark are the lead's in N7a, each with a test of
   the crossing case; N7e fills `getBreedOffspring` (R4.18).

### 9.5 What generation makes (R4.19, R4.20)

1. **The note writes in two rounds** when the chunk has entities: the entities file first,
   synced and renamed with its directory synced, then the blocks file; a blocks file then
   implies its entities. Tested on the file store with a disk that fails between the two
   renames (R4.19).
2. **A remake's chunk**, as loaded, copied and noted, is the new blocks with the stored
   entities (R4.20); tested with entities walked off and with entities that stayed.

### 9.6 The record (R4.21–23)

1. **The body holds the controls and the path** (R4.21), copied, so that N10's door
   behaviours can read another's path; the **mind** is the goal selectors' and the brain's
   state only, left out of copies.
2. **The module contract** adds (R4.22): stable ids for the mind's entries (an unknown id is
   dropped, a goal without an entry starts empty, so N9 adds goals without a format change);
   a decoded cache of copied mobs' `kind`, kept with copy updates, for typed reads across a
   line; a record that fails to decode or is of a later version is kept, its mob frozen and
   logged; the `EntityField`s `Body`, `MindEntry(id, version, bytes)`, `MindGone(id)`,
   `Kind(version, bytes)`, `Lived`, `Hops`; a `Set` of mind fields only, stripped for copies,
   still advances the chunk's copy chain.
3. **Versions** serve within a half and later steps, not across the halves, which refuse
   worlds anyway (R4.23).

### 9.7 Owners and order (R4.26, R4.27)

1. **The merge for an interaction at a line** (R4.26.1) is not built today (the
   coordinator knows `Near`, `Apart` and `Redstone`); it moves to the second half with
   N7n's measurement, and until then regions that follow players breed and follow across
   their line by effects, as pinned ones do (named).
2. **Light for a mob** (R4.26.2) is in the read view: the chunk's stored light where it has
   one; where a change left none, sky light 15 above the motion-blocking heightmap and 0
   below, block light 0; sky light darkened by the clock. Until N8.
3. **The harness comes with N7c** and N7c is accepted on it; N7e follows N7d too (R4.27).
4. **N7a is cut in two** (R4.H): **N7a1** the body, the read view, the draws and outputs,
   the module contract and the `lived` migration (what N7b and N7c need); **N7a2** faces,
   removals, the clock, the breeding protocol and the store's sides (what N7d needs),
   built beside N7b and N7c.

## 10. The fifth review (of section 9 and the plan then stated), and what it changed

An independent review (R5, 21 findings) of section 9 and of the statement of the plan that
followed it, which section 11 now replaces whole.

1. **A fresh take-in commits the entity `Whole`**, after its reset and raise (R5.1): the
   reset is written, so a crash never restores a mixture. The sim's four cases (R5.2):

   | Case | Resets | Raises | Commits | Sensors run |
   |---|---|---|---|---|
   | `Fresh` | yes | yes | `Whole` | yes |
   | `Moved` | no | yes | `Set(Hops, Lived)` | yes |
   | `Merged` | no | no | `Lived` only | yes |
   | `restore_loaded` | no | no | nothing | yes |

   The per-key `entity_memory` is dropped in every case (it is not state); the sim's side is
   the lead's.
2. **The round's rule, not the note's** (R5.3): `sync` puts no chunk's first blocks file in
   place until that chunk's entities file is in place and its directory synced, in two
   rounds whenever such a chunk is pending (a note, a save merged into a pending note, the
   restore's `apply`); the cost (two pairs of syncs at the second half's checkpoints in new
   land) is measured in N7n on the real disk. A note's first round may write an entities
   file over one already there, safe because a load never reads an entities file without its
   blocks file; the comment says so. Both paths tested on a disk that fails between rounds.
3. **A "continued" mark** (R5.4) beside the fresh mark: set at the release of a lent chunk to
   its pinned holder and when a use ends while a copier exists; the next use that begins
   from none consumes it without setting fresh. Commit-thread memory, lost at a store
   restart (which errs to continued). Neither rule touches uses. 9.1.4 is withdrawn.
4. **`Gone` counts** (R5.5) from the record's writer with `hops` at least the record's, or
   from any region with `hops` above it. Tested: a player who leaves mid-hand-over leaves
   the world list, and the remaining sleepers skip the night.
5. **Offline marks** (R5.6): at a split, only once both parts' first ticks are taken and only
   for players neither names; at any first tick, never a player whose latest note is another
   region's with higher `hops`, nor one in a hand-over (the let-go note carries `let_go`);
   after a store restart every region's first tick (ADR-0020's row 19) marks again.
6. **Faces and tombstones** (R5.7): no removal goes to a region the same note sends the
   player's new face; a region keeps per player the highest sequence it applied and one
   tombstone, the highest; a face is taken at a sequence at least the tombstone's and above
   the face's; a tombstone drops faces strictly below it.
7. **Breeding** (R5.8): the partner's region applies the asker's reset at once if it holds
   the asker when it breeds (looked up among all its entities and the arrivals taken); the
   asker stops its navigation and its `BreedGoal` when it asks, as the game's goal ends after
   `breed()`; each request has a number its answer names; an asked animal counts as not in
   love for others' `canMate` and `canContinueToUse` and refuses every request. Named: an
   answer later than 40 ticks, and acceptance on the asker's love as of its request.
8. **A `Set` of mind fields only is left out of the copy chain** (R5.9): `changed` is not
   called for it; the copier never sees it, and the next update names the change before.
9. **Section 11 lists every part and owner** (R5.10), **brings the first half's comparisons
   into N7f** (R5.11: a summoned mob, a hit, breeding, `set_time`), cites the plan's 7.10
   (R5.12), and names what R5.13 lists.
10. **The body is entries with stable ids** (R5.14, R5.15, R5.21), each with a durability
    class, written by N7a1 as one table (section 11.3); copies leave out the entries marked
    so.
11. **The read view** (R5.16) answers `is_loaded` (land or copies), orders a box query as own
    entities, copies' entities, held players, faces (named against the game's insertion
    order), and reads light (9.7.2).
12. **A local-effect queue** (R5.17): writes to other entities and blocks in a tick go as the
    same messages as the effects across a line, applied in order right after the acting
    entity's tick and before the next entity's; so breeding and a hit take one path whether
    or not a line is between.
13. **The module contract** (R5.18, R5.19, R5.20) is section 11.3's.

## 11. The plan as it stands

This is the plan; sections 2 to 10 are its reasons, and where they differ it holds.

### 11.1 The step in two halves

N7 is built in two halves, in the agreed order (N8 follows the second). **The first half**
builds the living body, the AI's machinery, players beyond the region, one world clock with
sleep, the farm animals from spawn eggs, and measures the AI's record. **The second half**
builds every other creature, the golems, villagers and monsters' bodies, the animals and
structures' entities made with each chunk, and the merge for animals at a line. Each half
refuses older worlds (the store's format and `FEATURES_VERSION` raised), ends in tests from
the specification by someone who built none of it, and has its "what to try" written into
the roadmap.

### 11.2 Decisions in force

- **Scope** (decision 1, 7.9): every creature with its AI and interactions; the three golems;
  villagers with the core and idle activities and their schedule's rest; monsters and other
  mobs from eggs and structures as bodies that float, stroll and look; water creatures, bats
  and axolotls only from eggs until N9's spawning.
- **The AI's state is durable and never reset** but at a fresh load, which is the game's own
  reload (7.2, 8.2, 9.1, 10.1): the body's and the mind's entries in the record; the mind
  (the selectors' and the brain's state) left out of copies; sensors' lists not durable and
  remade at every take-in in that tick.
- **Two counters** (8.2.2, 9.1.5): `lived`, the ticks ticked, keys the entity's stream and
  every stamp and is lowered only by a crash's restore; `fresh_at` gives `tickCount`; the
  game's counters are stamps against `lived`, those others read committed when they cross
  (7.2.5); `noActionTime` a stamp written at a hurt and when the nearest player leaves 32
  blocks (7.2.6).
- **Randomness** (decision 7, 7.2.8): the entity's stream keyed by (key, `lived`), a
  sub-stream with its own salt keyed by (key, `lived`, game time) with a count for draws on
  an entity outside its tick; the level's draws on an entity's behalf at their place in its
  stream; particle draws made and discarded; generation's draws the game's exactly; HotSpot
  maths through `libm`; the game's fixed hash orders kept, the ones that change between runs
  fixed (named).
- **Paths exact** (decision 8, 9.7.3), from extracted path types, judged by the jar's own
  path finder run inside the official server.
- **Players beyond the region** (7.3, 8.3, 9.2, 10.4–6): faces in the store's player records,
  sequenced, streamed to the copiers and the holder of the face's chunk, removed by
  tombstones; the departed face; `StayNote::Gone` and its rules; the world's players
  published once a second; offline marks.
- **One world clock** (7.4, 8.4, 9.3): the game time and each `world_clock`'s state,
  ratcheted, published, kept durably by the store, carried by commits outside the opaque
  state; `set_time` as the game sends it; sleep as the game's; sculk and hoppers on it.
- **Across a line** (7.5, 8.5, 9.4, 10.7): reads limited to land and copies, "unknown" for
  remembered things; the effects of 11.4; breeding by the partner's region with the asked
  mark; the merge for interactions at a line in the second half.
- **Generated entities** (7.6, 8.6, 9.5, 10.2): keys from the chunk with the top bit set;
  taken once (a blocks file means taken, by the round's rule); remakes keep stored entities.
- **The player** (7.7, 8.3.6): attack strength, experience, shoulders, hurt timestamps, sleep
  and the respawn point in the state, place and transfer, relative counters rebased at
  arrivals and merges.

### 11.3 N7a1, the contracts the delegated parts build on (mine)

1. **The record.** `EntityData::Mob { body: Vec<Entry>, mind: Vec<Entry>, kind: (version,
   bytes) }`, an `Entry` being (stable id, version, bytes). `EntityField`s: `BodyEntry`,
   `BodyGone(id)`, `MindEntry`, `MindGone(id)`, `Kind`, `Lived`, `FreshAt`, `Hops`. The codec
   in `clustine-format` writes bytes length-prefixed; postcard carries them as bytes.
2. **The body's entries**, one table in the code, each with its class (committed on change,
   a stamp, derived, or per tick with the reason it is exact) and whether copies carry it:
   vitals (health, hurt time and direction, death time, invulnerable time, last hurt amount);
   attributes (instances that exist, base values, modifiers); equipment and drop chances;
   the head's and body's yaw; the movement inputs (`xxa`, `yya`, `zza`, jumping, speed);
   each control's wanted state; the path, whole when made, and its progress (index, timers)
   apart; combat (target, aggressive, last hurt by and its time, last hurt by a player and
   its time); flags (persistence, `NoAI`, left-handed); the leash's home; `noActionTime`'s
   stamp; absorption waits for N9.
3. **The mind's entries**: one per selector (the goal selector's and the target selector's
   running set and locks), one per registered goal slot (by slot, not class: a rabbit's three
   `AvoidEntityGoal`s are three), one per brain memory, sensor and running behaviour (the
   brain's encoding fixed in N7j); an unknown id is dropped, a slot without an entry starts
   empty.
4. **The tick's skeleton**, in the game's order (brief 2.0.12), with the borrow pattern (the
   mob `&mut`, the rest `&`) and the local-effect queue (10.12).
5. **The read view**: blocks from land and copies, air beyond, `is_loaded`; entities in a box
   in the order own, copies', held players, faces; light (9.7.2); "unknown" for remembered
   things beyond.
6. **The draw API**: the stream and the sub-stream, the level's draws at their place, the
   particle draws discarded.
7. **Outputs**: sounds, entity events, swings, damage events and the `Shown` of a type, in
   neutral form; a frozen mob reported as an event the worker logs.
8. **The module contract**: one match in one file dispatches a type id to its module; the
   hooks in the skeleton's order (`register_goals` with each slot's id,
   `custom_server_ai_step`, `mob_interact`, `finalize_spawn`, `breed_offspring`,
   `fresh_load`, `shown`, `counters_others_read`); every older version of its bytes read; a
   decoded cache of copied mobs' `kind` for typed reads across a line, one accessor for own
   and copied neighbours.
9. **`lived`**: `Entity::age` becomes `lived`, stepped at the end of the entity's tick (a
   tick draws at the `lived` it began with), saturating; `fresh_at` beside it; an item's
   own age an offset from `lived`, which a merge changes, so `entities_ageing` writes `Lived`
   only; `first_tick` tests read `tickCount == 0`; carts, boats, stands, falling blocks, TNT
   and dispensed entities on the same stream; the edge's reports carry `Shown` for mobs, not
   their record.
10. **The extraction**: items' default components, default attributes, entity loot tables in
    26.3's form, spawn eggs' types, each state's path type and `isPathfindable`, memory
    types with a codec, `isValidSpawn` and spawn placements, entity, block, fluid and damage
    type tags.

### 11.4 N7a2, the store's and the edge's contracts (mine)

Faces, tombstones, the departed face, `StayNote::Gone` and `Has`'s `let_go`, `Discard`'s
player and `hops`, the world's players, offline marks; the clock in `RegionState` and the
commit, its ratchets, publication and durable home; the fresh and continued marks and `Took`;
the four take-in cases; the mind out of copies; the effects (first half: breeding with its
asked mark and numbers, `Given { player, hand, stack }`, the hit's `killed_by_player`, a
conversion across a line, a spawn egg on a mob across a line; second half: the alert, the
copper golem's flower, the bee and its hive, the pet's teleport); the generated-chunk rules
(the round's rule, remakes) with a test generator; the key layout and region blocks refused
at 2^31; the player's new state, place and transfer; the wire ids' reuse with the ping
quarantine; ADR-0020, 0025 and 0028 amended.

### 11.5 Parts

First half, in order:
- **N7a1** and **N7a2** (mine), N7a2 beside N7b and N7c once N7a1 is on `main`.
- **N7b** (delegated): the living body (brief 1) on the skeleton; the edge's living packets
  and each type's `Shown` mapping (mine).
- **N7c** (delegated): goals, controls, sensing, paths (brief 2, 3), accepted on the path
  harness (mine).
- **N7d** (mine): faces in regions, the clock in regions, `set_time`, sleep with its bed,
  pose, leave-bed input, respawn point and messages; a leash held from a face; `PlayerInside`
  retired once the plate tests pass with faces; sculk and hoppers on the clock.
- **N7e** (delegated, after N7b to N7d): farm animals (brief 4) but riding.
- **N7f** (mine): the measurement (a pen of 100, 20 sheep with wheat with the faces' share, a
  flight over eggs' animals, a merge with animals, a crash's restore of 500 mobs; the log, the
  copies' updates, the tick) and the encoding adjusted from it; the comparisons with the
  official server (section 7.10: a summoned mob with data and a rotation, a hit, breeding's
  packets, `set_time`).
- **N7g**: tests from the specification.

Second half: **N7h** tamed and ridden, pig riding, the logout on a mount (riding mine);
**N7i** wild and water; **N7j** brains, with the brain's encoding from its own measurement;
**N7k** golems, villagers, monsters' bodies; **N7l** made with the chunk (the pool's step
delegated, the store and keys mine), the format raised; **N7n** the merge for interactions at
a line, measured (merges and splits with animals; the round's two syncs on the real disk) and
compared (7.10: chunks' animals, structures' entities, the region random after a village
piece, paths); **N7o** tests from the specification.

### 11.6 Named differences

**Of the whole step**: a mob's counters after a crash are as of its last record, and a
departed face is forgotten; a load after a store restart, or of a chunk given back from a
lend or whose use ended while another region copied it, counts as continued; another mob's
mind is not read across a line; long path ranges plan shorter near a line, and the path-type
cache is not kept; remembered things beyond the copies are "unknown", kept; the nearest
player beyond the copies and the world's players are a second late; faces go with copies
when regions uncouple; a box query orders entities by Clustine's order, not the game's
insertion; carts and boats pass through a player beyond a pinned line; a parrot lands only on
a player its region holds; foxes' and pandas' pick-ups across a line, copper golems' chests
beyond their region's land, mob effects, a copy's entity counted for `isPushable`; breeding
across a line makes the child at the partner, accepts on the asker's love as of its request,
and an answer later than 40 ticks lets the asker breed again; the world's day runs at the
fastest region's pace, time is never turned back, a camel's pose across a line may be a second
off, other regions' sleepers wake a second late; weather is always clear; HotSpot's `sin`,
`cos`, `atan2` and `pow` through `libm`; a fixed order where the game's iteration changes
between runs; the "visible" range per region; a horse's menu closes at a line; name tags and
renamed eggs name nothing, buckets of fish carry no fish (no components); a spawn egg on a
spawner sets its entity but spawns nothing until N9; frogs have no prey and no tongue until
N9; generation's camel pose, difficulty and Halloween fixed; a remade chunk gets no template
entities; worlds are refused at each half; water creatures, bats and axolotls come only from
eggs until N9; villagers do no work, sleep in no bed and trade nothing until N10; monsters do
not attack or burn until N9.

**Of the first half only**: seeded worlds have no animals; eggs of types not yet built make
nothing; thrown eggs hatch nothing; a saddled pig is not ridden; regions that follow players
do not merge for animals at their line; light is the stored light, or the sky's where a
change left none, until N8.

### 11.7 The owner's questions (the recommendations are taken; each can be overruled)

1. The day passes, with sleep in beds; time is never turned back.
2. Weather stays clear until a later step.
3. Monsters from eggs are bodies that stroll (second half); before that their eggs do nothing.
4. Item components (name tags, a bucket's fish, a shulker box's contents, a renamed egg) are a
   step of their own after N7.
5. Villagers wander by day and rest at night, without work, beds or trades, until N10.
6. Water creatures come with N9's spawning, as the agreed order has it.
7. Worlds are refused at each half.
