# N7k: golems, villagers and monsters' bodies

Draft of 2026-10-21, for an independent reviewer before anything is built. The game's rules
are those of `n7k-brief.md` (cited as K: K 3.3 is its section 3.3, K 0.7 its finding 7),
read from the 26.3 jar; the dumps are in `/home/claude/reference/n7-javap/` and, for the
classes K section 9 found missing, `/home/claude/reference/n7k-javap/` (59 classes dumped
on 2026-10-21 from the jar `cargo datagen` keeps, nothing downloaded), which K's section 11
reads before this plan is reviewed. Cited as B: `n7-brief.md`; P: `n7-plan.md`; L:
`n7l-plan.md`.

Where it sits: P 11.5 names N7k "golems, villagers, monsters' bodies"; P 11.2 its scope
("the three golems; villagers with the core and idle activities and their schedule's rest;
monsters and other mobs from eggs and structures as bodies that float, stroll and look");
P 11.6 "villagers do no work, sleep in no bed and trade nothing until N10; monsters do not
attack or burn until N9", which L 9 item 12 amended: burning in daylight is N7k's. L 5.4's
D3 (the structures' entities) builds on N7k's bodies and comes after it.

Where things stand (`main` at `8f8a9db`): N7j is built; of N7l the keys, the store, the
creatures' `finalizeSpawn` by reason and group (D1) and a chunk's spawning (D2), the worlds
before it refused. The copper golem exists (N7j), built from its pattern; the zombie horse
and zombie nautilus exist and do not burn.

## 1. What a player sees when N7k is done

All 21 types of K have their egg, and each makes its mob as the game's egg does (K 4),
with its `finalizeSpawn` draws (K 5) and its synched data (K 6).

### 1.1 Golems

- **An iron golem** built from four iron blocks and a carved pumpkin or jack o'lantern,
  upright or lying down, as the game finds the pattern (`BlockPattern`, dumped now): the
  blocks go with their break particles, the golem stands at the bottom of the middle column,
  player-made, with no draws (K 0.7). It strolls, looks at players, holds out a poppy to a
  villager in daylight about once in 8000 asks (400 ticks), is repaired by an iron ingot,
  never drowns, and fights monsters: any `Enemy` but a creeper within 16 (every fifth tick)
  or one it bumps into (one in 20), walking at it and hitting for 7.5 to 21.5 with the
  game's fling upwards. Its damage sound plays when its cracks change (below 0.75, 0.5 and
  0.25 of its health, strictly, K 11.6); the client draws
  the cracks from the health. It does not defend villages (reputations, N10) and does not go
  back to one (no villages, B 8.15).
- **A snow golem** built from two snow blocks and a pumpkin, with the same finding. It
  leaves one layer of snow wherever it walks and a layer can rest, melts in the Nether and
  the seven warm biomes, is hurt by water, strolls avoiding water, looks at players, loses
  its pumpkin to shears (a carved pumpkin dropped). It targets any monster within 16 and
  **throws nothing**: the shoot sound plays and no snowball flies (named, K 1.6, until N9's
  projectiles; open question 2).

### 1.2 Villagers and the wandering trader

- **A villager** from an egg is unemployed, adult, its type by the biome at its feet (seven
  skins). It floats, strolls by day and looks at players and mobs (B 7.14.2's reduced
  brain: the core and idle activities); at night it stands and looks (the schedule's rest
  without a bed, open question 4). Clicking it shakes its head with `villager_no` (unhappy
  for 40 ticks), as the game's unemployed villager does; a baby the same. It picks up
  bread, carrots, potatoes, beetroots and their seeds and wheat into its eight slots. An
  egg used on a villager makes a baby of a type drawn as the game draws it.
- **A wandering trader** from an egg floats, avoids zombies and illagers, panics when hurt,
  strolls and looks. Clicking it does nothing until N10's trades (open question 5): the game
  always opens its screen with offers, which Clustine has not.
- A village's villagers and golems come with D3, after N7k.

### 1.3 Monsters' bodies

The zombie, husk, drowned, zombie villager, skeleton, stray, bogged, parched, creeper,
spider, cave spider, witch, pillager, vindicator, evoker, guardian and elder guardian, from
eggs (and, with D3, from structures: a swamp hut's witch, a mansion's illagers, a
monument's elder guardians, an igloo's zombie villager, ocean ruins' drowned).

- Each has its own goals but those of a target and an attack (**no target, no attack**:
  N9), and so does what the game's do without one (K 11.13 item 4): a skeleton runs from
  wolves within 6 and, burning in daylight, flees to shade and keeps out of the sun; a
  creeper runs from cats and ocelots; a spider from an armadillo that is not rolled up; a
  zombie, husk, zombie villager or drowned walks to turtle eggs within 24 and tramples them.
  Only the creeper, the spiders, the witch, the illagers and the guardians float: a zombie or
  a skeleton sinks and walks the bottom, as the game's (K 3.9, 11.2). A zombie whose eyes
  are under water turns into a drowned 901 ticks later (K 11.5), a husk into a zombie, and a
  skeleton in powder snow into a stray after 441 ticks. A drowned walks to water by day and
  sinks in it, and swims up and out onto land at night (K 11.1).
- **Undead burn in daylight** (K 3.3): the zombie, zombie villager, drowned, skeleton, stray
  and bogged, and the zombie horse and zombie nautilus already built, by the game's tag, at
  the game's hours and light, with the game's chance; a helmet protects and wears. The husk
  and the parched do not.
- What they wear and hold is the game's at the world's difficulty (section 3): at easy, a
  skeleton's bow and a pillager's crossbow, a zombie's iron sword, spear or shovel one time
  in a hundred, a vindicator's axe, a drowned's trident one time in 16, fishing rod one in
  27 and nautilus shell one in 33 (whatever the difficulty, K 11.1), nothing enchanted, and
  no armour. A spider is ridden by a skeleton one time in a hundred.
- A creeper swells and explodes 30 ticks after a player uses flint and steel or a fire
  charge on it (radius 3, blocks broken as `mob_griefing` lets them, K 11.3), with
  Clustine's explosions; with no target it never swells otherwise. The spider climbs walls,
  the witch drinks
  a healing potion when hurt (the item, the sound and the slowness; the potion heals nothing
  until mob effects, open question 8), the guardian flops on land and its spikes hurt a
  survival attacker (none in Clustine), the elder guardian curses no creative player.
- Monsters near a bed keep a non-creative sleeper awake (K 3.1; Clustine's players are
  creative, so the rule is built and never meets anyone yet).

### 1.4 The difficulty

A world has a difficulty, **easy** by default as the game's dedicated server, set by the
command line (`--difficulty`) and sent to clients (`change_difficulty`, locked). It decides
the special multiplier of every `finalizeSpawn` (K 0.4). At easy the multiplier is always 0
(B 1.11's `DifficultyInstance`: the effective difficulty never reaches 2), so no
enchantment is ever drawn and Clustine needs no enchantment registry; at normal and hard it
does, and so the other difficulties are refused until the enchantment providers are built
(named; open question 1). The difficulty's id also sets ranged uncertainty and the
skeleton's attack intervals, which N9 reads.

### 1.5 Left out, and named

- Attacks, targets of monsters, projectiles (the snowball, arrows, potions, the evoker's
  fangs and vexes): N9. The snow golem therefore neither hurts nor pushes anything: a
  snowball's 0-damage hit is what keeps zombies off it in the game (K 11.10).
- Trades, professions, work, beds, gossip, raids, patrols, the trader's despawning and
  invisibility: N10, N9.
- Mob effects: the husk's hunger, the cure of a zombie villager, the witch's potions, the
  elder's curse.
- Rain: nothing is wetted by it; the snow golem is not hurt by it.
- Difficulties other than easy (1.4).

## 2. The shared parts

### 2.1 The `Monster` layer (K 3.1)

`Family::Monster` in `body/ladder.rs`: experience 5 (a type may say otherwise: the guardian
10), the `hostile` sound source and sounds of swimming, splashing and falling, `noActionTime`
+2 in light above the magic value 0.5 (+2 always for a raider), the walk target value
`-(magic - 0.5)`, the loot only with `mob_drops` (on), never despawning in N7 (the
spawner's despawning is N9's; an egg's mob is persistent as the game's), and
`isPreventingPlayerRest` in `sleep.rs`'s monster check, which N7d left empty.

### 2.2 Burning in daylight (K 3.3)

In `Mob.aiStep`'s place in Clustine's tick (`mobs/tick.rs`, after the body's step, before
the loot pick-up), for every type in `#burn_in_daylight`: the clock's day by the `day`
timeline (`monsters_burn` from 23460 to 12542, as `bee.rs` reads `bees_stay_in_hive`), the
light's magic value at the eye in `f32`, the mob's own `nextFloat()` whenever the light
passes, wetness (water now, powder snow now or last tick), the sky at the eye; then the
helmet's `nextInt(2)` wear or `igniteForSeconds(8)` scaled by `burning_time`. `living.rs`
does the rest already. The zombie horse and the zombie nautilus gain it with everything
else; their tests that a mob does not burn are changed.

### 2.3 Equipment at spawn (K 3.2)

`populateDefaultEquipmentSlots` and the enchantment chances in `mobs/making.rs`, each type
calling them as the game does: the armour draws (copper between leather and gold, 26.3), the
weapon, the `nextFloat()` before each further slot. At special 0 the armour never comes,
and each enchantment's `nextFloat()` is drawn and passes never.

### 2.4 Conversions (K 0.20; `ConversionTracker`, dumped now)

A type's conversion of itself into another mob in place (`Mob.convertTo`, K 11.5): a new
record of the other type with exactly what K 11.5 lists as copied (equipment and drop
chances, name, persistence, the baby flag, the fire flag but not the fire ticks, the
passenger moved to the new mob and the new mob seated on the old one's vehicle, the leash),
**not** the health (the new mob starts at its maximum), air or attribute modifiers; **no
`finalizeSpawn`**: the zombie's step is `handleAttributes(CONVERSION)` on the new mob (four
of its own draws), the skeleton's none. The new mob is added before the old one goes, in
the same commit, and the level event plays at the new mob's cell unless it is silent. The
timers are `ConversionTracker`'s: 600 + 301 ticks for a zombie (601 + 301 once it has left
water), 140 + 301 for a skeleton in powder snow, the skeleton's counted before its tick; a
timer is saved and survives a restore. It is the region's (`Did::Convert` exists for a player's use, `types.rs` 872; this generalises it).
A conversion happens where the mob is held. The new mob has a new key from the region's
block and the old key goes, as a death and a birth: the edge shows a removal and an add, as
the game sends both.

### 2.5 A type's own hit (K 0.8)

A hook `MobType::do_hurt_target` beside `melee_hit`, defaulting to it: the iron golem's
amount draw, entity event 4, the attack sound, and **the fling**: `Local::Hurt` gains
`fling: Option<f32>` (0.4), applied by whoever holds the target **only if the hurt was
taken**, with the target's own knockback resistance (K 7). Across a line it rides in
`EntityUse::MobHit`, whose format gains it (one byte of presence and an `f32`).

### 2.6 Building golems (`BlockPattern`)

`CarvedPumpkinBlock.trySpawnGolem` and `BlockPattern.find` ported in
`region/golem_patterns.rs`: the three golems' patterns (the copper golem's hand-written
shape moves onto it, which mends its named difference), every orientation in the game's
order (K 11.7: 27 corner cells, x fastest; 24 pairs of forwards and up in `Direction`'s
order, so a golem is built lying down and upside down too); the `~` cell taken as air
(**Inferred**, K 11.7), which the comparison 6.2 settles with a torch at a shoulder; the cells cleared row by row with level event 2001, the golem placed, the neighbours
updated. A golem is built only where all its cells are the region's own land: clearing
cells beyond a line would need every effect to land before the golem stands, which the
effects' model does not promise. Named, as N6's multi-block structures at a line are
(`docs/groundwork/n6-plan.md`); a player building across a pinned line meets it, rarely.

### 2.7 `VillagerData` on the wire (K 0.15)

`DataShown::VillagerData { kind, profession, level }`, the edge's writer for serializer 18,
the types' and professions' registry ids from the datagen (`villager_type` and
`villager_profession` registries, in `registries.json` order).

### 2.8 Things read as data

`#burn_in_daylight`, `#villager_picks_up`, `#candidate_for_iron_golem_gift`,
`#accepts_iron_golem_gift`, `#cannot_support_snow_layer`, `#support_override_snow_layer`
(tags extracted already); `snow_golem_melts`'s seven biomes as a sorted list with the
dimension's flag; `shearing/snow_golem` and `shearing/bogged` extracted; the biome to
villager type map from `VillagerType` (by hand from the dump: 33 biomes, plains for the rest),
checked against the official server (section 6).

## 3. The types

Each type's module (`mobs/types/<type>.rs`) has its synched data, saved data,
`finalize_spawn`, goals or brain, sounds, use, egg and template hook, as K gives them and K
11 for the classes dumped now. What N7k gives each beyond K's tables:

| Type | Goals or brain in N7k | Special |
|---|---|---|
| iron golem | all of K 1.3 but the village defence (an empty goal in its slot) | 2.5's hit; repair; flower; sprint particle draws |
| snow golem | K 1.4's, the ranged attack throwing nothing | snow trail; melting; water hurt; shears |
| villager | B 7.14.2's reduced brain | refusal; picks up food; egg breeding by its rule |
| wandering trader | K 2.7's but the trading and the potions' use goals (empty in their slots) | refuses (open question 5) |
| zombie, husk, zombie villager | no float; stroll, look; the turtle egg goal (K 11.8) | burning (not the husk); the drowning conversion; baby; the chicken jockey at spawn |
| drowned | K 11.1's: to water by day, up and to land at night, stroll, look, the turtle egg goal; `DrownedMoveControl`, its water travel and swimming pose, `AmphibiousPathNavigation` | burning; trident, rod and shell at spawn; the zombie nautilus it rides from ocean ruins (D3) |
| skeleton, stray, bogged, parched | no float; `RestrictSunGoal`, `FleeSunGoal`, avoiding wolves, stroll, look (K 11.2) | burning (not the parched); the bow; a skeleton freezing into a stray; the bogged's shearing (K 11.2: its own drops and sound, `shearing/bogged` extracted) |
| creeper | float, avoiding cats and ocelots, stroll, look (K 11.3) | ignition by flint and steel or a fire charge, the swell and the explosion |
| spider, cave spider | float, avoiding armadillos, stroll, look (K 11.4) | climbing: a type hook for `onClimbable` from the synched flag set by `horizontalCollision` each tick, and `WallClimberNavigation`; the skeleton jockey (the caller adds it); effects drawn only on hard (none at easy); the cave spider draws nothing at spawn |
| witch | float, stroll, look | the drinking draws and healing drink |
| pillager, vindicator, evoker | float, stroll, look | leader draw; equipment |
| guardian, elder guardian | restriction, stroll, looks | flop; spikes; the elder's curse (survival only: none) |

## 4. What crosses a line

As K 7, with these decisions:

1. **The golem's hit** goes as `EntityUse::MobHit` with the fling (2.5).
2. **The snow trail** at a line: a `SetIf` effect for a cell in another region's land,
   conditioned on air and a snow layer surviving there, read by the holder; the `block_place`
   game event with it (ADR-0025).
3. **The zombie's chicken** at spawn: a chicken found across a line is mounted by N7j's
   mount across a line, which this plan assumes built (it is, `69e1c4f`). The zombie is
   not a player: the grant is for a player only. **Decided**: a mob mounting a mob across a
   line is not done; the search reads only the region's own chickens, named (a baby zombie
   from an egg within five blocks of a line, beside a chicken beyond it, rides nothing).
4. **Conversions** happen where the mob is held (2.4); the new mob is the region's.
5. **The golem's targets** are keys read from copies, a tick old (ADR-0028 section 3).
6. **Golem patterns** wholly in the region's own land (2.6).

## 5. The draws

K 5's table, with K 11.12's for the types dumped now. Mobs made inside `finalizeSpawn` (a
zombie's chicken, a spider's skeleton, a drowned's zombie nautilus) are added by the egg's
path and by D3's in the game's order, vehicle and passenger together. R is the caller's (an egg: the player's
stream aside; a structure: the run's region random); O the mob's own stream. The order is
the game's in every row; the tests (section 7) check each type's draws by a counting
`Draws`, as D1's did.

## 6. Comparisons with the official server

On a quiet machine, `#[ignore]`, with the jar:

1. Each type summoned by an egg at a fixed spot: its packets (spawn, data, equipment,
   attributes), the type and profession of a villager in three biomes.
2. An iron golem and a snow golem built from blocks, upright and lying: the blocks' events,
   the golem's spawn packet and position.
3. A zombie at noon in the open: the fire flag within the game's expected time, and a
   helmeted one never.
4. `change_difficulty` at login.

## 7. Tests from the specification

Delegated to someone who wrote none of the code, from K and this plan: each type's egg, data
and draws; burning (time, light, helmet wear, water); golems built in every orientation and
refused at a line; the iron golem's fight with a zombie, also across a pinned line (the
fling only where the hurt is taken); the snow trail across a line; a villager's refusal, its
float and its night; the drowning conversion and a restore in its middle, a baby zombie on a
chicken converting riding it, a skeleton freezing into a stray; a witch drinking; a creeper
lit by flint and steel exploding on the 30th tick; a spider climbing a wall; a skeleton
fleeing a wolf and the sun; a zombie trampling a turtle egg; the draws of every type's egg
against K 5 and K 11.12 by a counting `Draws`.

## 8. The division of work

The lead fixes first (one commit): `Family::Monster`, burning's hook, `do_hurt_target` with
the fling and its format, conversion, `DataShown::VillagerData`, the difficulty's setting,
`golem_patterns.rs`'s interface, the eggs' table. Then, in parallel, each in its own
worktree and crates of its own:

- **K1** golems: iron and snow golems, the patterns (port of `BlockPattern`), the copper
  golem moved onto them.
- **K2** villagers and the trader: the reduced brain, type by biome, refusal, pick-up,
  breeding by egg, the trader's goals.
- **K3** the zombie family and the drowned: conversions, baby, jockeys, equipment.
- **K4** skeletons (with their sun goals and freezing), the creeper (with its ignition on
  the region's explosion), the spiders (with the climbing hook and `WallClimberNavigation`).
- **K5** the witch, the illagers and the guardians.

Not delegated: the fling across a line and conversions (ordering in the commit). Then the
tests (section 7), the comparisons (section 6) and the roadmap.

## 9. Open questions, each with my recommendation

The brief's section 11 (the classes dumped on 2026-10-21) answered K 10's questions 7, 9 and
10 and raised one more, 12.

1. **Difficulty**: easy by default, the only one accepted until the enchantment providers
   are ported. *Recommended*: so; the game's dedicated server starts at easy.
2. **The snowball**: a sound and nothing thrown until N9. *Recommended*: so; a snowball
   without projectiles' collision would be a second, worse projectile model.
3. **The iron golem fighting monsters**: now, since zombies in villages are ordinary play.
   *Recommended*: now.
4. **The villager's night**: stands and looks (the idle package not run). *Recommended*: so,
   until N10's beds.
5. **The trader's click**: nothing (`CONSUME`, as an empty villager's). *Recommended*: so;
   an empty trading screen is a seam a player meets at once, a refusal is not.
6. **Floating monsters**: each type's own goals. *Recommended*: so (the game's zombie sinks).
7. **The drowning conversion**: now (the drowned is dumped). *Recommended*: now.
8. **The witch's potions**: drink, apply nothing. *Recommended*: so, named.
9. **The types dumped now**: in N7k. *Recommended*: so, from K 11.
10. **Golems lying down**: `BlockPattern` ported. *Recommended*: so.
11. **Burning for the zombie horse and zombie nautilus**: now. *Recommended*: now.
12. **The creeper's ignition** by flint and steel or a fire charge: built now with
    Clustine's explosions, or the item refused (named)? *Recommended*: built now; a player
    with flint and steel near a creeper meets it at once, and the explosion exists.

## 10. The review (2026-10-21), and what the plan is now

An independent review found nine defects, thirteen risks and seven nits. Where this section
and the ones above differ, this one holds. The classes it found still missing
(`ProjectileUtil`, `ProjectileDeflection`, `EntityHitResult`, `BlockStatePredicate`) were
dumped into `n7k-javap/` the same day, and `CarvedPumpkinBlock`'s bootstrap methods
(`javap -v`) show the iron golem's `~` to be `BlockStateBase::isAir`: air, read, not
inferred.

**The villager**

1. **It panics, and goes for food** (finding 1): the core package's `VillagerPanicTrigger`
   and `GoToWantedItem`, the `PANIC` activity (`VillagerCalmDown`, moving away from the
   nearest hostile and from what hurt it), the sensors `VillagerHostilesSensor` and
   `HURT_BY`. A villager by a zombie from an egg, or hit by a player, runs, as the game's.
2. **Its night is the rest package's runnable part** (finding 2): `ValidateNearbyPoi` and
   `SleepInBed` find nothing; `RunOne` of `SetClosestHomeAsWalkTarget` (no home: nothing),
   `InsideBrownianWalk` (under a roof it walks about), `GoToClosestVillage` (no village:
   nothing) and `DoNothing`, with its draws; then the minimal looks. Question 4's answer is
   this, not "stands and looks".
3. **No lead** (finding 8): `can_be_leashed` false for the villager and the trader.
4. Its types: **29** biomes named, plains for the rest (finding 23).

**Mob effects, the few N7k needs** (finding 3): a mob's record gains a short list of
effects, each a kind, an amplifier and the tick it ends at (ADR-0028's derived counter),
with their real effect where it is one rule: **fire resistance** (no fire or lava hurt),
**water breathing** (no air lost), **swiftness** (its speed modifier, +20 % a level,
added and removed with the effect), **invisibility** (the shared flag, for the trader's
potion at night, finding 28). **Instant health** heals `4 << amplifier` at once and keeps
no record. So the witch drinks as the game's, its `hasEffect` checks hold and its draws
follow the game's; the trader drinks invisibility at dusk and milk at dawn. Every other
effect (the husk's hunger, weakness, the spider's on hard, the elder's curse) stays named.

**Despawning** (finding 4): no "never". Monsters from eggs despawn by the game's rule
(`checkDespawn`, already Clustine's for `Family::Other`, which `Family::Monster` keeps),
the raiders by `PatrollingMonster.removeWhenFarAway`, the zombie villager by its own.

**The difficulty** (findings 5, 21, 22): the world's, **easy** by default, stored with the
world (its identity names it, as ADR-0021's generation identity does, so every worker reads
the same and D3's structure mobs are a function of it) and fixed for a world: a client's
`change_difficulty` and `lock_difficulty` are refused. The edge sends `change_difficulty`
with `locked = false`, as a new dedicated server does. Only easy is accepted until the
enchantment providers are built. The three places that assumed normal (`fire.rs` 15 and 42,
`navigation.rs` 101–110, `neutral.rs` 12) read the world's difficulty, and their tests are
changed; at easy fire spreads more slowly. Nothing in N7k reads the effective difficulty
beyond the special multiplier, which at easy is 0; eggs keep passing it so, said in the
code.

**Burning** (finding 6): a type hook `sun_protection_slot`, the head by default and the
body for the zombie horse and the zombie nautilus.

**Halloween** (finding 7): never, as generation (L 3.6): `clustine-sim` reads no clock, and
the date as a durable input is not worth it for one day a year. Named: on 31 October the
game's zombies and skeletons from eggs draw once or twice more and a quarter wear a
pumpkin.

**Items without components** (finding 9): `ItemStack` has none, so the ominous banner of a
patrol leader from an egg (one in 17 pillagers, vindicators and evokers), the witch's
potion's contents (the client draws an empty bottle) and a helmet's wear from the sun cannot
be held. The draws are made; the banner is a plain white banner, the potion a plain potion,
the wear lost. Named, until item stacks carry components.

**Mobs riding mobs** (finding 10): built, the lead's: a mob passenger controls its mob
vehicle (`Mob.getControllingPassenger`), whose move, jump and look goals stop
(`updateControlFlags`); a zombie mounting a chicken makes it a jockey (it lays no eggs and
may despawn). The chicken jockey, the spider jockey and a drowned's zombie nautilus ride as
the game's. An egg's mob made in another region (`send_made`) searches no chicken; a mob
and the vehicle or passenger made with it cross as one unit (finding 11). "The region's own
chickens" are those of the region that makes the zombie.

**The snowball** (finding 17, open question 2): built in N7k, the first projectile, from
`Projectile`, `ThrowableProjectile`, `ThrowableItemProjectile`, `Snowball` and
`ProjectileUtil`: its flight, its hit of a block or an entity (a 0-damage hurt with its
knockback; 3 to a blaze, none here), its particles' entity event, gone at the hit. Thrown by
the snow golem and **by a player** (a creative player's snowball, which today does nothing,
is ordinary play too). Its hit across a line is an entity effect, as a mob's hit is; the
snowball itself crosses a line as any entity. No exception is asked of the owner.

**The division of work** (finding 12): the lead's first commit holds, besides 8's list, the
21 types' modules registered as stubs, the equipment of 2.3, the `Enemy` predicate, a
type's `sound_source` (the 78 places that hard-code the neutral source read it), the body
hooks (`on_climbable`, `is_pushable`, water travel and the swimming pose, pushed by fluids,
`can_freeze`, `sun_protection_slot`), the egg's path adding vehicles and passengers, the
shearing tables, mob effects, mobs riding mobs, and `convert_to` (built on the tadpole's
growing up, `region/frogspawn.rs` 131 and `Local::GrowUp`). K3 calls `convert_to` and
builds nothing of it. The snowball is **K6**.

**At a line**

- **The snow trail** (finding 13): the golem's region checks the snow's survival on its copy
  and sends `SetIf { expected: air }`, ADR-0025's own; a cell placed twice in one tick is
  placed once; the `block_place` game event goes only with a placement made in the region's
  own land, and is named at a line.
- **Golem patterns** (finding 14): the first match in the game's order is taken; if it
  touches another region's land nothing is built, and no later match is tried. Tested.
- **The fling** (finding 18): applied inside the holder's hurt after its own knockback; it
  rides a new effect tag, `USE_MOB_HIT_FLUNG`, so the effects the log keeps are read as
  before.
- **New crossings** (finding 16): the guardian's spikes hurt an attacker across a line
  through `hit_by_mob`'s hook, as an entity effect back; a zombie tramples a turtle egg
  across a line by `SetIf` expecting the egg; the iron golem's poppy to a copper golem across
  a line is named (rare).
- **The creeper's explosion** (finding 19): `ExplosionInteraction.MOB`, drops decaying,
  blocks only with `mob_griefing` (on), across a line as TNT's; the creeper discarded with
  no death and no loot.

**Counters** (finding 15), each in the mob's record and resumed by a restore: the
conversion's two times (`ConversionTracker`'s, standing still while dead or `NoAI`), the
creeper's `swell` and `old_swell` (which the game does not save; Clustine's record holds
them, so a restore mid-swell goes on), the witch's `using_time`, the villager's unhappy
counter, the golem's attack and flower ticks, the effects' end ticks.

**The tick after a conversion** (finding 20): the old mob's tick is not run once it has
converted; what it would have done (a sound, a push) went with a mob the clients are told
is gone. Named.

**Nits** (24 to 29): the golem's search is `nextInt(3) == 0` each tick (`goals.rs` 1453);
the guardians swim, they do not float; a conversion's add is sent before its removal; the
skeleton's `canFreeze` false is in section 3; section 5 cites K 11.12, and the spider's
skeleton is added by the caller after the spider (`addFreshEntityWithPassengers`) while the
chicken and the nautilus are added inside `finalizeSpawn` before the mob.

**The open questions now**: 1 easy only, as above; 2 the snowball built; 3 golems fight now;
4 the rest package's runnable part, with panic and food; 5 `CONSUME`; 6 each type's own
goals; 7 the conversion now, the lead's; 8 the witch's potions with the few effects; 9 the
types dumped now; 10 `BlockPattern` ported, `~` air; 11 burning now, with the body slot; 12
the creeper's ignition now.

## 11. What building it changed (2026-10-26)

1. **A mob rider does not steer its mob vehicle until N9.** The game's rider steers by using
   its vehicle's navigation and move control as its own (`Mob.getNavigation` and
   `getMoveControl` return the controlled vehicle's) while the vehicle's move, jump and look
   goals stop (`updateControlFlags`). In Clustine each mob's tick has its own record only,
   and the rider's tick would have to write its vehicle's path and controls. Until N9 gives
   monsters targets, a zombie's goals are only strolling and looking, so a chicken jockey
   that walks by the chicken's goals looks much the same; with N9's targets it would not.
   Named; section 10's "Mobs riding mobs" holds for the seating, the companions and the
   units.
2. **Companions** (the lead's contract): `Spawning.companions`, through which a type's
   `finalizeSpawn` makes a mob beside it (`making::companion`) or rides one already there
   (`Companions::nearby`, `ride`); the egg adds a vehicle before the mob and a rider after
   it. An egg's mob made in another region's land makes none (named).
3. **Part B** found the copper golem stands at the pumpkin's cell (`getBlock(0, 0, 0)`),
   not at the bottom of the middle column; the `~` cells are cleared too, with air's id;
   and every key but the space fails on a cell the region cannot read.
4. **Part C** found a drunk potion's healing is an effect of one tick, healed at the next
   `tickEffects`, not at once; water breathing refills air under water, as the game's does
   for anything that keeps its air there.
5. **The difficulty** is a constant, `Difficulty::WORLD`, rather than a stored setting:
   with only easy accepted every worker reads the same; it becomes a setting of the world's
   meta when a second difficulty is allowed.
