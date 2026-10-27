# N9: monsters, projectiles and natural spawning

Draft of 2026-10-26, for the owner and an independent reviewer before anything is built;
revised the same day after the review (section 18, which says what changed and where).
The game's rules are those of `n9-brief.md` (cited as M9: M9 0.4 is finding 4 of its
section 0, M9 4.2 its section 4.2), read from the 26.3 jar, as `n9-dumps.md` settles its
inferred claims against the classes dumped since (cited as D9: D9 1.2 row 4, D9 5.5; where
D9 corrects M9, D9 holds, and this plan says so where it matters). The dumps are in
`/home/claude/reference/n9-javap/` (107 classes) and the earlier `n5-` to `n8-javap/`,
cited as "n9 AbstractArrow 938–1250" (the class's short name and the lines of its dump).
A5, A22, A25, A28 and A29 are ADR-0005, ADR-0022, ADR-0025, ADR-0028 and ADR-0029; P7k
and P8 are `n7k-plan.md` and `n8-plan.md`. Code is cited as it is at `99732c9`, paths
under `crates/clustine-sim/src/` unless another crate is named.

Where it sits: the roadmap's next item after N8 ("then monsters and natural spawning
(N9)"), and `n-plan.md` section 4: "Monsters, projectiles (arrows, snowballs, eggs,
tridents), burning in daylight, natural spawning with both caps, difficulty". Burning,
the snowball, the monsters' bodies and the difficulty were built in N7k (P7k sections 10
and 11); light in the simulation is N8, whose last steps are under way. N9 reads N8's
light (`region/light.rs` 247–272: the darkened raw brightness, `max_local_raw_brightness`)
and assumes N8 done before its spawning step.

Where things stand: monsters have bodies and goals but target and attack nothing
(`monster_ai.rs` 46–105 and `tamable::Idle` in five types, M9 0.4); the only projectile is
the snowball, hard-wired in `region/thrown.rs` (`snowball_kind()` 74–77, damage 0, the
hit's entity event 3, player power only); item stacks carry no components
(`clustine-world/src/entity.rs` 11–16, `clustine-protocol/src/item.rs`); nothing uses an
item over time (the edge drops `RELEASE_USE_ITEM`, `services/edge/src/play.rs` 433–464);
players have no effects; nothing spawns at runtime (only with a chunk, from eggs, by
breeding and conversion); the store publishes the clock and the world's players once a
second (`worldstore/src/lanes.rs` 2178–2193) and no counts; structure starts live only
in the worldgen service's memory (`services/worldgen/src/stages.rs` 51–81); the edge
sends no living flags (index 8), no effect packets, no effect particles, and drops
`DataChanged` and `EntitySound` (`fanout.rs` 3347).

What N9 changes that a world keeps: the store's format (item stacks with components, in
the store's own encoding and in containers' NBT, a player's effects and time since rest,
the new projectiles' records, a chunk's structures, the world's game seed), raised step by
step, in each step that changes the shape of a record a world already has (section 12;
new data tags need none, since a world made before has none of them); **worlds made
before are refused**, as at every raise before. The worker's `STATE_FORMAT` (16 now,
`services/worker/src/lib.rs` 524) rises likewise in every step that changes the shape of
`RegionState` or of a delta (section 18, finding 18).

---------------------------------------------------------------------------------------

## 0. What is decided, and why

The lead's answers to M9's twenty questions (M9 15), as decided, with the reasons, and
where reading D9 and the code refines them or this plan disagrees.

1. **Projectiles.** Arrows (plain, tipped, spectral, and the mobs': the stray's, bogged's
   and parched's are tipped arrows on the wire, since `Arrow.addEffect` writes the potion
   component, D9 1.1 "Arrow"), tridents (players' and the drowned's, with loyalty), eggs,
   snowballs (built), splash and lingering potions with the area effect cloud, experience
   bottles, ender pearls, firework rockets, evoker fangs, the llama's spit, and dispensers
   of all those the game's dispenser shoots; the bow, the crossbow and the trident in a
   player's hand and a mob's. *Refined by D9*: a firework rocket flies without any
   component (flight 1, 10 to 21 ticks, a harmless pop: D9 1.2 row 17), and
   `FireworkRocketItem.use` works **only while gliding** (D9 3.8). Clustine has no elytra
   flight (the edge parses `START_FALL_FLYING` and nothing acts on it), so a rocket from
   the hand is named, **not built**; a rocket placed on a block (`useOn`, which is what a
   creative player does with one within minutes), shot from a crossbow and from a
   dispenser are built. The llama's spit is built (fully dumped, M9 3.8; today a sound
   with nothing flying, `llama.rs` 633–642). Tridents and pearls are dropped by a
   dispenser, as the game's (D9 2.3).
2. **Item components**: a small typed set, its own first step (section 3). The lead's
   list (`potion_contents`, `potion_duration_scale`, `charged_projectiles`,
   `enchantments`, `use_cooldown`, `fireworks`) and, *added by this plan*, `damage` (the
   mob's crossbow wears by one a shot, D9 3.5; the helmet's wear from the sun that P7k
   named), `intangible_projectile` (how a creative player's arrows become CREATIVE_ONLY,
   D9 3.2, and what a creative crossbow's charge holds), and the four the ominous banner
   needs (`banner_patterns`, `item_name`, `rarity`, `tooltip_display`: the patrol leader
   of decision 16, and every leader made from an egg since N7k, P7k section 10 "Items
   without components"). `fireworks` is in, not "if": the creative tab's rockets of flight
   2 and 3 carry it, and a rocket's colours and damage are its explosions. *Added after
   the review*: `suspicious_stew_effects` and `ominous_bottle_amplifier` (the tab's stews
   and bottles, eaten and drunk: decision 5 of section 18), `chicken/variant` (an egg's
   chicks, 4.4). Components a client sends outside the set are dropped, as all are today
   (question 1), and what a player meets of that within minutes is named (1.4).
3. **Using an item**: one use state on the living body (hand, item, ticks remaining),
   ticked as the game's (`updatingUsingItem`, `onUseTick` every use tick, `releaseUsing`,
   and `useOnRelease`'s extra use tick that loads a pillager's crossbow, D9 3.4), folding
   in the wandering trader's drinking, the living flags (index 8) sent, kept in the mob's
   record. **The witch stays off it** (finding 11): it keeps its own `DATA_USING_ITEM`
   (index 17) and `usingTime` (n7 Witch 150–219), and never sends living flags. A player's use and its release (`PlayerInput::ReleaseUse`,
   the use-item packets) are the lead's, since a player's state moves at a hand-over.
   *Refined*: the crossbow's use lasts 72 000 ticks and loads in `onUseTick` on the 26th
   use tick while still held (D9 3.1, 3.4), not at the release.
4. **An arrow's hit across a line**: the arrow is gone at a hit on an entity across a
   line, the holder hurting as it can. *Refined by D9 1.2 rows 1, 4 and 11*: a refused
   arrow does not "bounce off at × 0.2" but turns 170 to 190 degrees at a tenth of its
   speed and lives on (`deflect(REVERSE, …, 0.2)`), so the seam at a line is an arrow
   that vanishes where it would have fallen back; a burning arrow sets the fire **before**
   the hurt and undoes it on a refusal, which the holder does in that order; a piercing
   arrow, which hits every entity along its step (D9 1.2 row 7), ends at its first hit
   across the line (named). The enderman's refusal needs no answer (the arrow flies on,
   D9 5.6): the sender judges it by the type. Where regions follow players they merge for
   it (decision 17). **The trident needs none of this**: it bounces by REVERSE (0.02, 0.2,
   0.02) whether or not the hurt took and sets `dealtDamage` before it (D9 1.2 rows 2 and
   14), so it stays in the sender's region and only its hurt crosses, **except at an
   enderman**: the bounce and `trident_hit` run only `if
   projectileReceivesSideEffectsOnHit(took)` (n9 ThrownTrident 376–399), which the
   enderman answers with `took` (n9 Enderman 973–976), so a trident it refuses flies on
   with `dealtDamage` set; the sender judges that by the type, as for the arrow
   (finding 6).
5. **Steering a vehicle**: the rider's tick drives its controlled vehicle's navigation
   and move control in the same tick, a second view over the vehicle's record written
   back within the region's tick, with the real control flags, the stroll's
   `hasControllingPassenger`, `rideTick`'s body yaw and the vehicle's box in the attack
   reach (section 6). The lead's.
6. **Mob effects**: slowness, speed, poison, weakness, strength, regeneration, instant
   health and harming (the splash's factor, the undead inversion, a thrown potion's
   `indirectMagic`, D9 4), glowing, invisibility, fire resistance, water breathing, night
   vision, jump boost, slow falling, levitation, hunger (nothing on a mob, exhaustion on a
   player, which a creative player never feels), absorption and resistance, the types'
   refusals, and every effect a potion of the creative tab or a tipped arrow carries
   (section 5), **the four that act at a death or a hurt among them**: oozing, weaving
   and wind_charged, and infested with the silverfish it makes (decision 2 of section
   18). Intervals as D9 4 confirms them.
7. **Effects on players: in N9.** A player's effect store in the player's record, saved
   in their place and moved at a hand-over; `UpdateMobEffect` and `RemoveMobEffect`; the
   attribute modifiers the client is told of; glowing, invisibility and the effects'
   particles shown to others (section 5.4). Drinking a potion is in (the use state of 3).
8. **Difficulty**: easy only, every rule reading `Difficulty::WORLD`; the effective
   difficulty computed from the overworld clock (`getOverworldClockTime`, the day clock
   that sleep moves on: n7d ServerLevel 1336–1345) and the moon, with inhabited time 0
   (named). The `(0, 0)` that eggs pass becomes the computed value; generation keeps day
   0 and a full moon, which is a function of nothing but the chunk (A21), named
   (section 7).
9. **The target goals' placeholders** are all replaced at once, in the lead's first mob
   commit, with the stream changes and the tests that pinned them changed with it; M9
   0.4's five types that miss draws today are mended there.
10. **`alertOthers` across a line**: an entity effect, "take this target", to mobs in
    copies (section 8.3).
11. **Mobs**: the zombie family, skeletons and their kin, the creeper, spider and cave
    spider, the witch, the drowned, the guardians, the illagers (pillager, vindicator,
    evoker with the new vex), the slime, the enderman (block carrying and teleport its own
    part), the phantom (decision 16's spawner). *This plan adds the endermite* (one pearl
    in twenty makes one, D9 2.8: a player throwing pearls meets it within minutes; not
    dumped, question 4). **The sulfur cube is out**: its class alone is 2,377 lines of
    `javap` with its own archetype registry, item search and tempting, which is not small;
    its biome is generated (`sulfur_caves`), where it is 100 of the monster list's 311
    weight, so a third of the first picks there end the whole position (M9 0.26, D9 5.8),
    and its eggs are refused (question 3). The ravager and the illusioner are out (raids).
    **The silverfish is in** (decision 2 of section 18): the infested effect makes it,
    and with it come its goals (n9 Silverfish and its two inner goals) and infested
    blocks' `spawnAfterBreak` (an explosion's, as the game's; `InfestedBlock` is owed).
12. **Spawning's counts across regions**: as A28 section 8, refined by D9 5.1 and 5.2
    (section 9, as revised by findings 2, 4 and 30). The spawnable chunk count is computed
    by each region from the world's players' table (it is a function of players'
    positions alone); each region reports its loaded chunks' counts by category with its
    commits; the store keeps them **by region and chunk**, a region's full report
    replacing its whole set; it publishes to each region, with the players' table once a
    second, **the other regions'** totals by category and each player's local count over
    **the other regions'** chunks (the sum over chunks whose centre is within 128 blocks
    of them, which is the only use the game makes of per-chunk counts); a region adds its
    own chunks' counts as they are now, so nothing of its own is counted twice or late.
    The global cap filters the categories once, at the start of the spawning phase, as
    the game's `getFilteredSpawningCategories` does before the chunks are shuffled;
    `afterSpawn` raises the region's own counts within the tick, which the local cap reads.
    Nothing spawns before the first publication, nor after a store restart until every
    owned region has sent its full report. How late: about a second for other regions'
    changes, never for the region's own. *This plan disagrees in one detail with A28
    section 8's text*: the store publishes per-player sums, not per-chunk counts, since
    per-player sums are what is read, and are smaller; A28 is amended (section 13).
13. **A pack that steps into another region's land**: the attempt refused, as the game
    refuses a chunk that cannot tick; named (packs within about 20 blocks of a line are a
    little smaller on the line's side).
14. **Players for spawning and despawning**: the region's own players (live), the faces
    in its land and copies, and the world's players' table beyond them, for the nearest
    player, the local cap and the spawning chunks; until a table has come, nothing
    spawns and nothing despawns.
15. **Structures at runtime**: each chunk carries the starts of the five overworld
    structures with spawn overrides that reach it, with the boxes the overrides read
    (section 9.6). Generation writes them; nothing else changes them.
16. **Custom spawners**: phantoms (`PhantomSpawner`, D9 5.4: to creative players too,
    after 72 000 ticks awake; time since rest kept per player and reset by sleep) and
    patrols (`PatrolSpawner`, no raids needed, D9 5.4; the patrol goal, the leader and its
    banner) are in; cats, the wandering trader and the village siege wait for N10's
    points of interest, named. *Refined*: `PatrolSpawner` refuses a player within two
    sections of a village (`isCloseToVillage`), which reads points of interest Clustine
    does not have; until N10 the village structures' boxes stand in for it (question 8).
17. **The merge for targets across a line**: built where regions follow players (a mob
    whose target is across a line counts as work there), tested with a horde chasing
    villagers over a line both ways; pinned lines keep effects (section 8.5).
18. **The extras of a hit that took** (the zombie's fire, the husk's hunger, wear,
    `doPostAttackEffects`, the attack sound): *replaced after the review* (finding 5;
    decision 1 of section 18). They run where and when the hit lands, inside one region
    as across a line, drawing from a stream of the hit, `Random::of_hit(world, attacker,
    attacker's lived, target)` (8.2), which the hurt carries what it needs of, so the
    holder draws them as the attacker's region would. The draft's "drawn at the attacker
    as if the hit took" is gone. **A change to the game's order of draws, named**: the
    game draws the zombie's `nextFloat` from the attacker's own random within its tick;
    Clustine draws it from the hit's stream when the hit lands.
19. **Comparisons with the official server**: M9's list and D9's additions (section 11).
20. **Order of work**: the lead's first (components; the projectiles' variants; the use
    state and the release; the player's effects; the entity effects; the real target
    goals; the controlled vehicle; the counts and the merge), then delegated parts with
    files of their own (section 12).

*Added by this plan*: **natural spawning of every category**, not only monsters. The
game's spawner runs creatures (every 400 ticks), ambient, the water categories and
axolotls with the same code and caps, and the game's oceans fill with cod, squid and
dolphins and its caves with bats and glow squid within seconds of a player arriving;
generation makes only the creature category (`generation/spawning.rs` 39–121), so
Clustine's oceans are empty today. Every type of those lists is built (N7i), and their
spawn rules are dumped (n7 Bat, WaterAnimal, AgeableWaterCreature, TropicalFish,
GlowSquid, Axolotl). A player would notice an empty sea within minutes. With every
category comes each type's `getMaxSpawnClusterSize` (fish 8, schooling fish their school
size, horses 6, the wolf 8, the pillager 1, every other 4: n7 AbstractFish,
AbstractSchoolingFish, AbstractHorse, Wolf, Pillager, Mob) and every rule of D9 5.7 that
draws (9.3, 9.4).

*Added after the review*: **every world has a game seed** (decision 3 of section 18),
the flat world included: the seed given at the world's making, or for a flat world a
fixed default the server picks then, kept in the world's identity and carried to
regions beside the features' seed (9.4). Slime chunks and the biome at a block (the
game's zoomed lookup) read it, and the comparisons give both servers the same seed.

---------------------------------------------------------------------------------------

## 1. What a player sees when N9 is done

Every Clustine player is in creative. **No monster ever targets, keeps or hurts a creative
player** (M9 0.1, three locks, `sensing.rs` 132, 293, 439–444), and a creative player's
hit makes no monster turn on them nor alert others. So what a player meets of N9 is mobs
fighting mobs, what they shoot and throw themselves, and monsters spawning, burning and
wandering. Plainly:

### 1.1 Night falls and monsters come

- Within a minute of dark, around them in the open and in caves: zombies, skeletons,
  creepers, spiders and the odd witch and enderman; husks and parched in deserts, strays
  in snowy plains, bogged and slimes in swamps (slimes also deep in slime chunks), drowned
  in rivers and oceans, guardians at monuments, a witch at a swamp hut, pillagers at an
  outpost; zombie villagers now and then, a zombie horse in plains. Nothing in an ancient
  city or a trial chamber; nothing on mushroom fields or in the deep dark.
- They burn at dawn (built), and wander off and despawn when the player goes away
  (built, now judged by the world's players).
- Endermen carry blocks off and put them down elsewhere (`mob_griefing` is on) and
  teleport away from daylight and from arrows.
- Fish, squid, dolphins and turtles fill the seas; bats the caves; axolotls and glow
  squid the lush caves; animals come back to the plains over time.
- After an hour of play without sleep, at night under open sky above sea level, one or
  two phantoms appear overhead and circle (they cannot attack a creative player). After
  about 100 minutes of play, by day, a patrol of two or three pillagers led by one with an
  ominous banner appears 24 to 47 blocks away and walks off together.

### 1.2 Mobs fighting mobs

- In and around a village at night: zombies, husks, drowned and vindicators go for the
  villagers (who stay dead at easy: no conversion) and the wandering trader, pillagers
  shoot at them, and evokers raise fangs under them and summon vexes; the iron golem
  fights back and seeks monsters; skeletons shoot golems and flee wolves (they do not
  shoot them, finding 46); zombies, drowned and skeletons chase baby turtles on land,
  drowned axolotls, guardians squid and axolotls.
- A zombie's arms go up as it closes in, and it swings; a skeleton draws its bow, strafes
  and shoots every three seconds; a pillager charges its crossbow (shown loaded) and
  shoots; a witch throws potions that splash with their colour; a drowned raises and
  throws its trident; a guardian's beam shows for four seconds before it strikes.
- A creeper hit by a snow golem's snowball walks up to it and explodes. A witch targets
  only players (never a creative one), raiders it heals and what hurt it: a witch hit by
  a skeleton's stray arrow throws potions back at the skeleton.
- Arrows stick in the ground and in mobs (the arrows shown in a mob's body); a mob hit by
  a spectral arrow glows; a poisoned or slowed one shows the effect's swirls.
- A spider jockey moves as its skeleton wants, a chicken jockey chases what its baby
  zombie targets.
- An evoker by blue sheep turns them red.

### 1.3 Their own weapons

- **The bow**: drawn as long as the button is held (others see it drawn), released for an
  arrow whose speed follows the draw, critical at full power; their arrows can be picked
  up by any creative player and add nothing.
- **The crossbow**: charged with its two loading sounds and loaded on its 26th tick while
  held; shot with the next use; with multishot (an enchanted crossbow), three arrows; a
  rocket in the off hand is loaded and shot instead of an arrow.
- **The trident**: raised, thrown after at least half a second, coming back with loyalty.
- **Eggs** (chicks one time in eight, four one time in 256), **snowballs**, **splash
  potions** (the effect on whatever is within four blocks, the thrower included),
  **lingering potions** (a cloud that shrinks over thirty seconds), **experience
  bottles** (orbs), **ender pearls** (the thrower taken where it lands, with a one-second
  cooldown shown on the item, and one time in twenty an endermite), **firework rockets**
  placed on a block (fly and pop, in the colours of their stars).
- **Potions drunk, and every food and drink** with the game's sounds and crumbs (a
  creative player can eat anything): each effect on themselves from the item's
  `consume_effects` data (golden apples, rotten flesh's hunger four times in five,
  pufferfish, spider eye, raw chicken, poisonous potato, suspicious stews of the tab,
  milk clearing them all, honey taking poison away, an ominous bottle's bad omen, which
  does nothing without raids), and a chorus fruit's teleport: the icon and its timer,
  speed and slowness in how they move, jump boost and slow falling, levitation, night
  vision, invisibility and glowing seen by others; harming and poison do nothing to them.
- **Dispensers** shoot arrows, eggs, snowballs, potions, experience bottles and rockets.
- An arrow or a trident a skeleton aims at a golem that hits them instead turns back
  from them and falls; a creeper's explosion pushes them when they walk, not when they
  fly, and hurts nothing.

### 1.4 What they do not see, named

- Monsters attacking them: never, in creative (M9 0.1).
- A rocket from the hand (only while gliding with elytra, which Clustine does not have).
- Riptide (an enchantment on a trident used in water or rain: no weather, and the
  creative tab has no such trident).
- Raids, the ravager and the illusioner; the village siege, cats spawning in villages and
  the wandering trader's spawning (N10's points of interest).
- The sulfur cube (decision 11). (The silverfish is now in: infested potions and
  infested blocks an explosion breaks make it.)
- Weather: no thunder, so no skeleton trap and no lightning; rain does not put out a
  burning arrow.
- Zombies breaking doors, reinforcements, a cave spider's poison, a villager turned into
  a zombie villager: none of them happens at easy in the game either (M9 0.7).
- Inhabited time: a chunk's difficulty never rises with the time spent in it (at easy it
  would change the zombie's fire and the husk's hunger after more than thirty hours in
  one chunk).
- Components outside section 3's set, as today, and these are what a player meets of
  that within minutes: an enchanted book loses its `stored_enchantments`, a goat horn its
  `instrument`, a renamed item its name, dyed leather its colour, a painting of the tab
  its variant; and the first click in a menu after a creative player set such a stack is
  corrected by the server (its hash cannot match).

---------------------------------------------------------------------------------------

## 2. What D9 changes in M9, in one place

| M9 | D9 | Where in this plan |
|---|---|---|
| 0.3, 0.16: a refused arrow "× 0.2, may drop" | × −0.1, yaw +170–190 (one own `nextFloat`), fire restored, lives on; dropped only at motion² < 1e-7 | 4.2 |
| 4.1: `getManyEntityHitResult` is the spear's | the arrow's: every entity along the step, in order | 4.2 |
| 4.3: trident | flat 8.0f, no speed, no crit, no arrow count; `dealtDamage` before the hurt; bounces either way; water inertia 0.99 | 4.3 |
| 3.5: fangs | event 4 at warmup −1, damage at −8, gone 23 ticks after; ownerless `magic` | 4.8 |
| 15.1: rockets need `fireworks` | fly without it; from the hand only while gliding | 0.1, 4.7 |
| 3.1, 4.5: crossbow `charge + 3`, loads at release | 72 000; loads in `onUseTick` at progress 1; the end sound's level draw before the goal's `nextInt(20)` | 4.6, 6 |
| 4.6: dispenser position | centre + 0.7·facing **+ 0.1 in y**; rockets centre + 0.50001·facing | 4.9 |
| 4.4: splash distance | box to box (the potion's box at the hit, the target's inflated by the margin); `indirectMagic(potion, owner)`; level events 2007/2002 then 1054/1053 | 4.4 |
| 4.4: cloud | shrinks 0.005 a tick, duration scale 0.25, at the hit entity; no server draws | 4.5 |
| 4.4: pearl | 96 own draws on every hit, then the endermite's `nextFloat`; 5.0 damage; cooldown is `use_cooldown` | 4.4 |
| 4.4: experience bottle | `3 + nextInt(5) + nextInt(5)` is the experience, split into orbs | 4.4 |
| 4.4: egg | chicks take the egg's yaw, no `finalizeSpawn`; a failed fudge stops further chicks | 4.4 |
| 8.1: `spawnEnemies` | does not read the difficulty | 9.1 |
| 8.7: phantoms need survival | creative players too | 9.7 |
| 8.7: patrols from day 5 by a day test | the `early_game` timeline's `can_pillager_patrol_spawn`; patrols start by day | 9.7 |
| 8.4: slime rule | one float against `surface_slime_spawn_chance`, falls through to the slime chunk, whose `nextInt(10)` is always drawn | 9.4 |
| 8.3: entities in the spawn box | only those that `canBeCollidedWith` (boats, carts, shulkers) | 9.3 |
| 3.2: bow draws "the arrow's 2" | none from the bow; the mob's triangle only in `getMobArrow` | 4.6 |

D9's STILL UNKNOWN items: `Mob.finalizeSpawn`'s random is the level's (`ServerLevelAccessor
.getRandom`, n7 Mob 2645: the follow range's triangle, then the left hand's `nextFloat()
< 0.05`), which for natural spawning is the chunk's stream (9.3); the vex's flags are
index 16, the charge bit 1 (n7 Vex); the cloud's predicate and the fangs' size are taken
as D9 presumes, each checked by a comparison of section 11 (finding 58).

---------------------------------------------------------------------------------------

## 3. Item components

### 3.1 The type (the lead's, `clustine-world`)

`ItemStack { item: i32, count: i32, components: Components }`, where `Components` is
`Option<Box<Patch>>` (none for a plain stack, which is nearly every stack, so a stack
costs one pointer more) and `Patch` is a list ordered by kind of `Component` values and of
removed kinds:

| Component | Value | Read by |
|---|---|---|
| `potion_contents` | potion id (the `minecraft:potion` registry) or none, custom colour or none, custom effects (each an effect, amplifier, duration, ambient, visible, show icon), custom name or none | drinking, splash, cloud, tipped arrows, colours on the wire |
| `potion_duration_scale` | `f32` | the same (lingering 0.25, tipped arrow 0.125 by default: data) |
| `charged_projectiles` | a list of stacks (at most 1,024, n9 ChargedProjectiles 3–16) | the crossbow; the client's loaded look |
| `enchantments` | a list of (enchantment id by name, level) | the rules of section 4 that read one |
| `use_cooldown` | seconds, cooldown group or none | the pearl (D9 2.2), any item that has it |
| `fireworks` | flight duration, explosions (shape, colours, fade colours, trail, twinkle) | the rocket's lifetime and damage; the client's colours |
| `damage` | `i32` | wear: a mob's crossbow, a helmet in the sun |
| `intangible_projectile` | none (a marker) | the arrow's pickup rule |
| `banner_patterns`, `item_name`, `rarity`, `tooltip_display` | as the game's codecs | the ominous banner |
| `suspicious_stew_effects` | a list of (effect, duration) | eating a suspicious stew (5.5) |
| `ominous_bottle_amplifier` | `i32` (0 to 4) | drinking an ominous bottle: bad omen (5.5) |
| `chicken/variant` | a chicken variant by name | an egg's chicks (4.4) |

Registry values (a potion, an effect, an enchantment, a banner pattern, a variant) are
kept by name in the world's types and in the store, and turned into the registry's id
only on the wire, so that a game update that renumbers a registry needs no format raise
(question 1's answer).

**Where the types live** (finding 37). Two `ItemStack`s stay apart, as now, since
`clustine-protocol` depends on no crate of Clustine's and a worker never sees a packet
(A5):

- `clustine-world/src/components.rs` (with `components/`): the world's `Components`, the
  values above, their reads with defaults, and each component's **structure as the
  game's `Codec` has it** (one function a component, to a small value tree of maps,
  lists, numbers and strings), from which the containers' NBT (3.2) and the menus'
  hashes (3.3) are both made, so the structure is written once.
- `clustine-protocol/src/components/`: the wire's values and the game's **stream codecs**
  for the set, both ways, with the `data_component_type` ids generated by `cargo
  datagen` into `clustine-protocol/src/generated/` (as the packet ids are).
- **The edge** converts between them and validates against the registries
  (`clustine-data`: a potion, effect or enchantment that exists, a duration within the
  game's limits, a list within its maximum); the protocol crate checks only the shape.

Every read of a component reads the patch, then the item's default (the generated table
`clustine_data::ItemComponents`, which gains the defaults of the set from the datagen's
`reports/minecraft/components/item/*.json`: a lingering potion's 0.25, a pearl's
cooldown, a crossbow's empty charge, a rocket's flight 1, an egg's `chicken/variant`
(finding 50), and the `consumable` and `food` components of every item that has one,
which section 5.5 reads). A component the patch removes reads as absent.

- **Equality and merging.** Two stacks merge, stack in a slot, are picked up into one
  another and are moved together by hoppers only when item **and components** are equal
  (`ItemStack.isSameItemSameComponents`). Every site that compares `.item` to decide a
  merge goes through one function, `same_kind`; the lead's step lists them (hoppers,
  item entities merging, `Inventory.add`, the menus' clicks, dispensers' and droppers'
  stacking, the explosion's drop merging). A stack's maximum size reads `max_stack_size`
  from the defaults (not in the set: a patch cannot change it).
- **Not `Copy`.** `ItemStack` loses `Copy` (1,274 mentions in the workspace); the change
  is mechanical, made by the lead in one commit with the compiler's help, before
  anything else in N9.
- **Deterministic.** Lists in a fixed order (by kind; effects as given; enchantments by
  the registry's order), no hash maps, so `clustine-sim` stays as its `clippy.toml` wants.

### 3.2 In the store

Format 14, in N9a (section 0): a stack is its item and count, then the number of
components, each written as **its kind by name**, a length, and its value in Clustine's
own encoding (registry values by name), then the removed kinds by name. A kind added later
is a new name, read where it is known; so later components need no format raise (the
plan's answer to the lead's "fits without another format change").

**Every writer of a stack** (finding 15; the draft's "one function changes" was wrong).
N9a changes each, and its test (section 14) round-trips a stack with every component of
the set through each:

- `put_stack` (`clustine-format/src/entities.rs` 189–224, 585), for what it serves;
- the item entity's record, written inline (`entities.rs` 383);
- `EntityUse::Interact.held` and `Given` in the effects' format (`clustine-format/src/
  effects.rs` 170, 294);
- the player's place: `LoggedPlace` holds `Option<(i32, i32)>` tuples and is `Copy`
  (`log.rs` 255–260, 1088); it becomes stacks, loses `Copy`, and gains the effects and
  the time since rest (sections 5.4, 9.7) in the step that adds them (N9d);
- **containers in chunk files, which are the game's NBT** (`ItemStackWithSlot.CODEC`,
  `region/containers.rs` 296–338): each component of the set is written and read in the
  game's persistent NBT form (its `Codec` with NBT ops), from the structure function of
  3.1, so a potion in a chest keeps its contents at a save and a chunk written by the
  game reads. `docs/world-format.md` says so (section 13).

### 3.3 On the wire (the edge; the comparisons check it)

- **Clientbound** (`put_item_stack`, `clustine-protocol/src/item.rs` 19–31): the count, the
  item, the number added and removed, then each added component by its
  `data_component_type` registry id and its value by the game's stream codec, then the
  removed ids. Each of the set's codecs is written from the game's (`PotionContents`,
  `ChargedProjectiles`, `ItemEnchantments`, `UseCooldown`, `Fireworks`, `BannerPatternLayers`,
  text components for `item_name`), checked byte for byte against the official server
  (section 11).
- **Clientbound, read** (finding 16): Clustine's own reader of servers' stacks
  (`Reader::item_stack`, `clustine-protocol/src/item.rs` 39–51) fails today on any
  component, and a server sends components with no length before them, so a component
  cannot be skipped unread. N9a gives it the stream codecs' decoders for the set, and a
  component outside the set is a decode error that names its id (the tests and the bots
  meet only the set). Its readers are `SetEntityData` and equipment, the container
  packets, `items_oracle.rs` and the bots (`tools/botswarm`), which N9a moves onto it.
- **Serverbound** (`untrusted_item_stack`, 58–73): clients length-prefix every component,
  so the edge decodes those of the set and **drops the others** (question 1), and validates
  what it decodes (a potion id that exists, an effect duration within the game's limits, a
  list within its maximum), refusing the stack's components where they are malformed, as
  the game's codec would refuse them.
- **Hashes.** A click in a menu carries `HashedStack`s (`play.rs` 2617–2637), the
  component values' hashes (the game's `HashOps`, CRC32C over the codec's structure).
  Today `matches` needs none. The edge computes the game's hash of **every** component of
  the set (`potion_contents`, `potion_duration_scale`, `charged_projectiles`,
  `enchantments`, `use_cooldown`, `fireworks`, `damage`, `intangible_projectile`,
  `banner_patterns`, `item_name` as text, `rarity`, `tooltip_display`, and the three added),
  from the structure function of 3.1 (`services/edge/src/component_hash.rs`), so that a
  client's right guess about a potion in a chest is a match and is not corrected (a
  correction is invisible, but every click with such a stack would be one). Checked
  against the official server's own hash (section 11).
- `PlayerInput::SetInventorySlot` and `Throw` and `EntityUse::Interact`'s `held` carry
  stacks with their components; the creative tab's tipped arrows, potions and rockets keep
  them.

---------------------------------------------------------------------------------------
## 4. Projectiles

### 4.1 The records (the lead's, `clustine-world` and `clustine-format`)

`Entity.kind` already names the type (`entity.rs` 26–55), so the snowball's hard-wiring
(`snowball_kind()`, `region/thrown.rs` 74–77) goes and `make_thrown` takes the kind. The
records, each a new data tag of the entities' format (14 to 18; `DATA_THROWN` is 13,
`clustine-format/src/entities.rs` 94–106), added in N9b, whose records they are (finding
18: a new tag needs no format raise, since no world made before has one):

- **`Thrown`** (unchanged: stack, owner, left owner, has been shot) for every
  `ThrowableItemProjectile`: snowball, egg (the three), splash and lingering potion,
  experience bottle, ender pearl. Each kind's gravity, its hit and its sound come from a
  table by kind (4.4); the potion's contents are its stack's component.
- **`Arrow`** (boxed) for the arrow, the spectral arrow and the trident: pickup (0, 1, 2,
  n9 AbstractArrow$Pickup 81–96), the pickup stack, the weapon fired from (none for a
  trident, D9 1.1), owner, left owner, has been shot, base damage (`f64`), crit, pierce
  level, the pierced keys (at most `pierce + 1`, cleared at a block hit), the hit sound,
  **in ground** as the state hit and the `lived` it landed at (from which `inGroundTime`,
  the 1200-tick despawn and the tipped arrow's 600 are derived, A28 section 3's derived
  counters, so an arrow lying in the ground commits nothing), the shake's end, the fire
  ticks it carries; the spectral arrow's glowing duration; the trident's `dealtDamage`
  and its return count. Loyalty and foil are read from the pickup stack as the game's.
- **`Spit`**: owner, left owner, has been shot. No synched data (M9 3.8).
- **`Firework`**: the stack, life, lifetime, shot at an angle, attached to (always none:
  no gliding).
- **`Cloud`**: age as a derived counter, duration, wait time, reapplication delay,
  radius, radius on use, radius per tick, duration on use, the potion contents and
  duration scale, owner, and the victims with the tick each may be affected again (kept in
  the record, so that a restore mid-cloud does not affect again too soon; ordered by key).
- **`Fangs`**: owner, warmup and life as derived counters, the yaw.

A shot is made by one function, `make_projectile(kind, at, aim, power, uncertainty,
construction)`, generalised from `make_thrown` (`region/thrown.rs` 270–341): the key (its
own chunk's if the region ticks it, else `take_key` and `send_made`), the entity's own
stream for its UUID (2 draws) and its construction's draws, then `shoot` or
`shoot_from_rotation` (6), then added (the game's `spawnProjectile`). `Local::Shoot`
(`mobs/tick.rs` 216) carries a `Shot { kind, stack, weapon, aim, power, uncertainty,
from_mob: Option<f32> }`, `from_mob` being the `setBaseDamageFromMob` power whose
triangle the arrow draws from its own stream after its UUID (D9 1.1). The constructions'
positions (eye − 0.1, the crossbow's rocket at eye − 0.15, D9 3.4) are the caller's.

**What a hit or a spell makes where the region does not tick** (finding 23). One rule
for all of N9's makings: what lands in the region's own land is made there; what lands
in a chunk the region copies is made by `take_key` and `send_made` (its own draws made
by the maker, as `make_thrown`'s are), the holder adding it; what lands where the region
can read nothing is not made (named). So: the egg's chicks (at the egg, in its own land
but for an egg that hit an entity in a copy, whose position may lie across), the
experience bottle's orbs (`awardWithDirection`, 4.4), a lingering potion's cloud at a
hit entity in a copy, the evoker's vexes (its position `−2 + nextInt(5)` off in x and z),
the evoker's fangs (4.8), the phantoms (±10 and 20 to 34 up, 9.7) and the silverfish of
an infested mob. **Patrols** are the exception: the game asks `hasChunksAt(±10)` of the
patrol's position, and Clustine asks that every chunk within 10 blocks of it be the
region's own land, refusing the firing otherwise (named: a patrol is not made within 10
blocks of a line); a later member whose steps take it out of the region's land is not
made, as a member that fails `spawnPatrolMember` in the game's loop is not (9.7).

The hit test is shared: `targets` (`region/thrown.rs` 380–430: loaded chunks and copies,
then the region's own players) gains the **faces** of other regions' players in held and
copied land as standing boxes that can be hit and refuse every hurt (they are creative),
so that an arrow aimed past a player across a line turns back from them as it would in one
region where it can (the arrow itself is then the sender's, 4.2). A face's box is the
player's as the face shows them: 1.8 tall, or 1.5 crouching (the face's `sneaking`,
finding 49). `clip_blocks`, `margin` and `pickable_and_alive` are kept; arrows and the
trident add the game's `canHitEntity` (a player owner's arrow does not hit a player that
may not be harmed; not the owner's vehicle's passengers before it has left the owner).

### 4.2 Arrows (D9 1.1 throughout)

The tick in D9 1.1's order, which is not the throwables' and does not reuse their tick:
the motion read first; the stuck test; the shake; in water or rain the fire out (no rain
in Clustine); **in the ground**: a changed block (by reference to the state hit) and
`shouldFall` → three own draws × 0.2 and flying again, else the despawn at 1200 (the
trident's never while ALLOWED with loyalty), and no base tick (a burning arrow in the
ground keeps burning); **flying**: water inertia 0.6 (trident 0.99) before the move, the
rotation, `checkLeftOwner`, the clip and `stepMoveAndHit`, the 0.99 drag unless in water,
gravity 0.05 unless the step put it in the ground, then the base tick.

**`applyEffectsFromBlocks`** (finding 21; n9 AbstractArrow 572–664, called from
`stepMoveAndHit` and in the ground): on every flying step and every tick in the ground
the arrow runs `entityInside` of the blocks its box meets, as a walking entity does
(`region/pressed.rs`'s path for entities, which S2 calls, not edits). It is what lets an
arrow press a wooden button and keep it pressed (`ButtonBlock.checkPressed` looks for
arrows: `region/redstone.rs` 415–417 says today "there are no arrows", and its arm calls
S2's `arrow_on_button`, stubbed by the lead in N9b), a wooden pressure plate and a
tripwire, and what sets an arrow alight in fire or lava. A player meets an arrow on a
button within minutes.

`stepMoveAndHit`: every entity along the step (`getManyEntityHitResult(…, false,
false)`; the trident only the nearest, and none once it has dealt damage), sorted by
distance to each entity's position, hit in order until the arrow is dead or deflected;
with piercing, again from the new position. Each hit:

1. `j = ceil(clamp(speed · damage, 0, MAX_INT))` (with Power, `modifyDamage` from the
   `enchantments` component: the enchantment's data, section 4.10); then, in the game's
   order (finding 8; n9 AbstractArrow 938–1110, offsets 116–250): with pierce > 0, the
   pierced set first: **if it is full the arrow is discarded silently**, no damage and
   no crit draw; otherwise the target's id added; then a crit draws `nextInt(j / 2 + 2)`
   (own); then the owner's `setLastHurtMob(target)`, whatever the hurt's outcome (it is
   not one of the extras "if it took": a sender does it at once for a target in a copy).
2. The fire: a burning arrow sets the target alight for 5 s **before** the hurt.
3. The hurt, `arrow(this, owner)`:
   - **an entity the region holds**: taken → its arrow count (pierce ≤ 0), Punch,
     the post-hurt effects (the pickup stack's `potion_contents` scaled by its
     `potion_duration_scale`; spectral: glowing 200), the hit sound (own draw), gone
     unless it pierces; refused → the fire restored, REVERSE at 0.2 (own draw), and gone
     only below motion² 1e-7 (dropped if ALLOWED); the enderman (refuses projectiles and
     receives no side effects): the arrow flies on to the next entity;
   - **a player of the region or a face** (creative): refused, so REVERSE as above;
   - **an entity in a copy** (another region's): decision 4. The sender writes an
     `EntityUse::Hurt` (section 8.3) with the amount, the source, the fire to set first
     and restore on a refusal, the arrow count, the knockback and the effects to give if
     it took, and the hit sound with its pitch drawn now (own); the arrow is **gone**
     (discarded, no drop), unless the target's type is the enderman and it is no
     passenger (finding 25), which the sender knows from its copy: then the arrow flies on
     as above, and the holder sets the fire, refuses and teleports. A piercing arrow ends
     there (named).
4. A block hit: the block's own hook (`onProjectileHit`: a target block's signal, a bell,
   TNT lit by a burning arrow, a chorus flower, a pointed dripstone, a decorated pot; in
   another region's land a `SetIf` or the block's effect as ADR-0025's), the position
   moved back **per axis** by `signum(m) · 0.05` of the motion and the motion set to zero
   (AA 1317–1408; finding 8), the ground sound (own draw), in the ground, shake 7, crit
   and pierce off, and the hit sound reset to `arrow_hit` after it sticks. These hooks are
   the "waits for N9" of `region/thrown.rs` 24–28, and serve the snowball too: their
   signature (`projectile_hit(kind, pos, state, hit, owner)`) is fixed by the lead in
   N9b in `region/projectile_hits.rs`, which S2 then fills and S3 calls.

**The arrow count** of a living entity (data 12, n7d LivingEntity 411): a body part of
the mob, the count and the tick at which the next arrow goes (`20 · (30 − count)` ticks,
a derived counter), no draws.

**Pick-up** (`playerTouch`): in the ground (or with no physics) and its shake over, by a
player of the arrow's region whose box it is in (A28 section 7: pick-up only in the
player's own region): ALLOWED adds the pickup stack, CREATIVE_ONLY needs infinite
materials (any creative player, D9 1.2 row 13), the trident only by its owner or anyone
when ownerless; `TakeItemEntity` and gone. A creative player's own arrows are
CREATIVE_ONLY (their bow drew intangible ammo) and add nothing.

**The tipped arrow** in the ground for 600 ticks: entity event 0 and the pickup stack
becomes a plain arrow (D9 1.1 "Arrow").

**Armour stands**: an arrow or a trident kills one outright (`#always_kills_armor_stands`).

### 4.3 Tridents (D9 1.1 "ThrownTrident")

Before the arrow's tick: `dealtDamage` after 4 ticks in the ground; loyalty, an owner and
(dealt damage or no physics): its owner gone or a spectator → dropped if ALLOWED; a mob
owner within its width + 1 catches it; otherwise no physics, the return pull
(`y += d.y · 0.015 · L`, motion `· 0.95 + d̂ · 0.05 · L`) and `trident_return` once. The
owner's eyes are read where the owner is: the region's own player or mob, a face (a
player of another region, a tick old), or the world's players' table (a second old,
eyes at feet + 1.62); a trident returning across a line crosses as an entity and is
picked up in the owner's region (named: it may circle a second-old position if its owner
is beyond the faces). Its hit: 8.0f through `modifyDamage` (Impaling: from the data),
`dealtDamage` set **before** the hurt, `trident_hit` with no draw, REVERSE (0.02, 0.2,
0.02) with one own draw whether or not the hurt took, **but not at an enderman that
refused it** (`projectileReceivesSideEffectsOnHit(took)`, finding 6): there it flies on,
`dealtDamage` set, with no sound and no bounce. Across a line only the hurt is sent
(`EntityUse::Hurt`, no extras), the trident bouncing in the sender's region, or flying on
where the target is an enderman that is no passenger, judged by the type as the arrow's.

### 4.4 Thrown items (D9 2)

`tick_thrown` (`region/thrown.rs` 438–619) is kept, with the gravity by kind (egg and
pearl 0.03, potions 0.05, bottle 0.07, D9 2.1) and the hit by kind:

| Kind | Entity hit | Then (`onHit`), server only |
|---|---|---|
| snowball | built: 0 (3 to a blaze) | entity event 3, gone (built) |
| egg (three) | `thrown`, 0 | own `nextInt(8)`, on 0 own `nextInt(32)`; one or four chicks at the egg with its yaw, age −24 000, the stack's `chicken/variant` or the item's default (3.1), no `finalizeSpawn`; a failed fudge ends the loop (D9 2.6); entity event 3, gone |
| splash potion | — | the water behaviours (hurt the water-sensitive by `indirectMagic` 1, put out fire, rehydrate axolotls) in the unmoved box; with effects: each living entity in the box moved to the hit point inflated (4, 2, 4), box to box, proximity `1 − √d² / 4`, instant effects by `applyInstantaneousEffect` with the proximity, others `(int)(prox · dur · scale + 0.5)` unless that is 20 or fewer (`endsWithin(20)` is `duration <= 20`, n7 MobEffectInstance 373–385; finding 10: D9 2.4 and the draft said "under 20"); `douses_fire` at a block hit; level event 2007 or 2002 with the colour, then 1054 or 1053 unless silent; gone |
| lingering potion | — | the water behaviours; with effects a cloud (4.5) at the hit entity or the potion; the level events; gone (a water bottle makes no cloud) |
| experience bottle | — | level event 2002 with `−13083194`, 1053; experience `3 + nextInt(5) + nextInt(5)` (own, always) split into orbs by `awardWithDirection` (n7 ExperienceOrb 551–660; not in `orbs.rs` today, finding 23): at the hit, along the face or against the motion (D9 2.7), each orb's value by `getExperienceValue`, then `tryMergeToExisting` with its `nextInt(40)` (the level's random: the bottle's region's stream aside), else a new orb with the directed constructor (its reflection, offset and `unstuckIfPossible`); an orb whose place is in a copy is sent (4.1); gone |
| ender pearl | `thrown`, 0 | 96 own draws (32 × `nextDouble`, `nextGaussian`, `nextGaussian`); the owner, allowed (alive, not sleeping, same dimension); a player owner: own `nextFloat() < 0.05` (always drawn) and monsters spawning → an endermite at the player, with their rotation; the teleport to the pearl's position before this tick, keeping rotation, zero motion; fall distance reset; `enderPearl` 5.0 (refused: creative); `player_teleport` (D9 2.8); gone |

Each splash or cloud reaches entities of three kinds: the region's own (applied there),
those in its copies (an `EntityUse::Effects` to each, with the instances already scaled
and the proximity for the instant ones, section 8.3), and players: the region's own
(their effect store, 5.4) and **faces** of other regions' players (an `EntityUse::Effects`
addressed to the player, which the store delivers to the region that holds their stay:
section 8.3). The thrower is affected as anyone.

**The pearl's teleport** where its owner is a player of another region (a pearl thrown
over a pinned line): an `EntityUse::PearlLanded { to, endermite }` addressed to the
owner, known by their face or the players' table, which the store delivers to the region
that holds their stay (8.3); that region moves them
there (the player's position set, `PlayerEvent::Teleported`, and their hand-over follows
by the ordinary rule if the place is another region's land), hurts them by 5 (refused) and
makes the endermite at where they stood. The pearl's own draws are the pearl's region's.
A pearl whose thrower's stay is between regions when it lands teleports nobody (named); a pearl in flight when its thrower leaves the
game falls and teleports nobody (the game saves it with the player: named).

**The player's throws** (D9 2.2): `use` for each kind: the sound before the projectile
(the region's stream aside, one `nextFloat`), `spawnProjectileFromRotation` with the
kind's roll, power and uncertainty (egg, snowball, pearl 0, 1.5, 1.0; potions −20, 0.5,
1.0; bottle −20, 0.7, 1.0), nothing consumed in creative, and the `use_cooldown` component
where the stack has one: the player's cooldowns (a group or the item, with its end in the
**region's ticks**, carried as the ticks remaining in `PlayerTransfer`, at a merge and in
`PlayerState`, as `attack_reset` is: `state.rs` 99–105, `region.rs` 1718; decision 8 of
section 18, finding 1), `ClientboundCooldown` to their client. A running cooldown refuses
the use of its item or group (finding 51). They are in the region's state, which the store
keeps with the region, but **not in the player's place**, so a leave forgets them, as the
game, which saves none, does (finding 55 corrected the draft's "do not reach the store").

### 4.5 The area effect cloud (D9 2.5)

`serverTick` as D9 2.5: the duration, the waiting flag (data 9), the shrink by radius per
tick (data 8, the radius; **derived**, not stored: the radius at an age is a function of
the radius it was made with, the age and what each application took off, so the record
holds the radius as of the last application and a cloud commits only when it applies or
ends, rather than every tick for thirty seconds: finding 53), and every 5 ticks the victims dropped whose tick has come, then
each living entity in its box, in the region's order, horizontal distance within the
radius (the float square widened), each effect applied (instant at 0.5), radius on use
and duration on use. No draws. `EntityDimensions.scalable(radius · 2, 0.5)`. Across a
line: as the splash (4.4), each target in a copy or a face sent its effects; the victims
map counts those too, as in one region. The particle (data 10) is `entity_effect` with the
potion's colour. The predicate D9 leaves unnamed (`noneMatch(target::…)`) is taken as
`canBeAffected`, confirmed by the comparison of section 11.

### 4.6 Bows, crossbows and tridents in a hand (D9 3)

These are rules of the items, written once and called for a player and for a mob, **with
one exception both S4's and S7's briefs state** (finding 12): the bow's and the trident's
`releaseUsing` act only for a `Player` (`instanceof Player`, n9 BowItem, TridentItem), and
a mob shoots through its `performRangedAttack` alone; a skeleton or a drowned whose use
ends runs nothing of the item's, or it would shoot twice. The crossbow's rules serve both.

- **The bow**: `use` starts a use if the user has infinite materials or a projectile
  (off hand first); `releaseUsing` with `t = 72000 − remaining`, `getPowerForTime(t)`,
  nothing below 0.1, `draw` (Multishot from the data; `useAmmo` intangible for a creative
  player and every copy), `shoot` at `3f · f`, uncertainty 1.0, crit at full power, then
  `arrow_shoot` with the level draw after (D9 3.3).
- **The crossbow**: `use` shoots what is charged (`getShootingPower`: 1.6 if any is a
  rocket, else 3.15) or starts a use; `onUseTick` at progress 0.2 and 0.5 the loading
  sounds (their "played" flags shared by every holder in the server, as the game's
  singleton has them: D9 3.4; Clustine keeps them per region, named), at 1.0 `draw` into
  `charged_projectiles` and `crossbow_loading_end` with one level draw; `releaseUsing`
  loads nothing; `useOnRelease` true, so a release runs one more use tick.
  `performShooting` empties the charge and shoots each projectile at its angle (0, −10,
  +10), crit for a player, never a mob, a rocket at eye − 0.15 flying straight; a sound
  per projectile, a pitch drawn from the shooter's random for each after the first; wear 1
  (3 for a rocket), none for a creative player.
- **The trident**: `use` starts a use (not if its next damage breaks it, nor with
  Riptide outside water); `releaseUsing` after at least 10 ticks: `trident_throw` at the
  entity with no draw, `spawnProjectileFromRotation(…, 0, 2.5, 1.0)`, CREATIVE_ONLY for a
  creative player, who keeps theirs. Riptide: named (decision 1).
- **Potions and other consumables** (section 5.4): drunk over the `consumable`
  component's time (32 ticks; 1.6 s by default, 0.8 s dried kelp, 2 s honey), with
their effects at the end (5.5).

### 4.7 Firework rockets (n9 FireworkRocketEntity)

The construction's draws (two triangles, then `nextInt(6)` and `nextInt(7)` for the
lifetime `10 · flight + …`), the tick (×1.15 horizontally and +0.04 a tick unless shot at
an angle; the hit test on the move; the launch sound at life 0), the explosion at the end
of its life or at a hit (entity event 17, the `explode` game event). **Its damage is by
distance, not by hit** (finding 9; n9 FireworkRocketEntity 573–722): `5 + 2 ·
explosions` in full goes only to `attachedToEntity`, which is always none in Clustine;
every other living entity, the one it hit included, is in the radius loop: `distanceToSqr
≤ 25`, a clear clip from the rocket to the entity's feet or its middle (two tries), and
`(5 + 2n) · sqrt((5 − d) / 5)`; none without explosions. A block hit runs the block's
`entityInside` first (finding 52), then explodes. Made by `useOn` a block (the clicked
point moved 0.15 along the face), by a crossbow, by a dispenser (with `shotAtAngle` true,
n9 FireworkRocketItem 150, so it flies straight: finding 7). Its explosion's hurts across a line are
`EntityUse::Hurt`. Data 8 the stack, 9 attached to (empty), 10 shot at an angle.

### 4.8 Evoker fangs (D9 1.1 "EvokerFangs")

Made by the evoker's spell where `createSpellEntity` finds a floor (M9 3.5), in the chunk
of that place: the evoker's region's if it holds it, else `send_made` (a line of 16 fangs
across a line is ordinary). The tick: warmup down; at the first negative tick entity event
4 once; at warmup −8 every living entity in its box inflated (0.2, 0, 0.2) but its owner,
allies, the dead and the `Invulnerable` hurt by `indirectMagic(fangs, owner)` 6 (`magic`
without an owner); those in copies by `EntityUse::Hurt`; gone 23 ticks after the warmup
ran out. No server draws. Its size is the entity type's (0.5 × 0.8, from the generated
entity sizes).

### 4.9 Dispensers (D9 2.3)

`Behaviour::Projectile(kind)` (`region/dispensers/behaviours.rs` 76–121, 138–184) for
arrows (plain, tipped, spectral), the three eggs, snowballs, splash and lingering potions,
experience bottles and rockets (fire and wind charges wait for blazes and breezes: they
stay drops). It makes the entity only, so it reads nothing in front (`reads_front` false,
as the egg's). The position: the dispenser's centre + 0.7 · facing + (0, 0.1, 0); a
rocket's centre + 0.50001 · facing; `asProjectile` (an arrow ALLOWED with no owner),
`spawnProjectileUsingShoot` along the facing at the configuration's power and uncertainty
(finding 7, the draft had them swapped; n9 ProjectileItem$DispenseConfig$Builder 11–27,
ThrowablePotionItem and ExperienceBottleItem's `createDispenseConfig`, FireworkRocketItem
156–165):

| Projectile | Power | Uncertainty |
|---|---|---|
| default (arrows, eggs, snowballs) | 1.1 | 6.0 |
| potions and the bottle | 1.375 | 3.0 |
| rocket | 0.5 | 1.0 |

the stack shrunk, the sound level event 1002 (a rocket 1004). A dispensed rocket is made
with `shotAtAngle` true (n9 FireworkRocketItem 150). **The projectile's draws are its
own** (`entity_stream(key)`, as the armour stand's are, `behaviours.rs` 414): its UUID,
its construction's and `shoot`'s; a projectile's dispense draws nothing from the
dispenser's stream (`ProjectileDispenseBehavior` reads no level random). Tridents and
pearls stay drops.

### 4.10 Enchantments

26.3's enchantments are data (their effects in `data/minecraft/enchantment/*.json`). N9
reads the ones its rules read, each a hand-written rule from its file's values, through
one module (`items/enchantments.rs`): Power (`damage` on arrows), Punch (`knockback`),
Flame (the arrow lit 100 s), Infinity (`ammo_use` 0), Multishot (`projectile_count`,
`projectile_spread` 10), Piercing (`projectile_piercing`), Quick Charge
(`crossbow_charge_time`), Loyalty (`trident_return_acceleration`), Impaling (`damage`
against `#sensitive_to_impaling`), Unbreaking and Mending for wear only where it matters.
Riptide and Channeling are named. A mob's equipment enchanted at spawn is not reached at
easy (the guard's float is drawn and fails, D9 6 "enchantItemFromProvider").

---------------------------------------------------------------------------------------

## 5. Mob effects, on mobs and on players

### 5.1 The store (`mobs/body/effects.rs`)

The body part `Effects` (`effects.rs` 179) is kept, each layer an amplifier and an end in
the mob's `lived` (A28's derived counter), and gains the flags the game keeps per
instance (ambient, visible, show icon) and the hidden layers' order the game's
`MobEffectInstance.update` keeps. The same type is the player's store (5.4), with ends in
**the region's ticks** (not the world's game time, which jumps forward by up to a second
when the store publishes and differs between regions at a hand-over, `WorldClock::later`,
`api.rs` 1107–1112: finding 1), carried as the ticks remaining wherever the player moves
(decision 8 of section 18).

### 5.2 What each does (D9 4; n7 MobEffects for the modifiers)

| Effect | Rule |
|---|---|
| speed, slowness | movement speed ×(1 ± 0.2 / 0.15 · (amp + 1)), multiplied total (built for speed) |
| strength, weakness | attack damage +3 / −4 · (amp + 1) |
| poison | every `25 >> amp` (every tick at 0), `hurtServer(magic, 1)` while health > 1 |
| wither | every `40 >> amp`, `hurtServer(wither, 1)` at any health |
| regeneration | every `50 >> amp`, heal 1 below the maximum |
| instant health, harming | `4 << amp` healed or `6 << amp` hurt (magic, or `indirectMagic(source, owner)` from a potion or cloud), inverted for `#inverted_healing_and_harm` (`isInvertedHealAndHarm`, n7d LivingEntity 2449); a splash's proximity `(int)(f · v + 0.5)` |
| hunger | a player's exhaustion (`0.005 · (amp + 1)`: nothing in creative); nothing on a mob |
| glowing | shared flag bit 6 |
| invisibility | shared flag bit 5 (built) |
| fire resistance, water breathing | built |
| jump boost | the jump's power (`getJumpBoostPower`) and `safe_fall_distance` +1 · (amp + 1) |
| slow falling, levitation | the travel's gravity: slow falling 0.01 and no fall damage; levitation `(0.05 · (amp + 1) − motion.y) · 0.2` a tick |
| absorption | the absorption amount `4 · (amp + 1)`, lost first in a hurt |
| resistance | hurt × `(25 − 5 · (amp + 1)) / 25` **after armour** (finding 13: n7d LivingEntity 4769–4916 takes armour's absorption, then the magic absorption, resistance before protection, then absorption), skipped for `#bypasses_effects` and `#bypasses_resistance` |
| oozing (n9 OozingMobEffect) | when its bearer is removed as KILLED: `2` slimes of size 2 (the count clamped to `max_entity_cramming` 24 less the slimes within its box inflated by 2, not counting itself), each at its feet + 0.5 up with the yaw `nextFloat() · 360` of the level's random (the dying entity's stream aside), `TRIGGERED`; particle `item_slime` |
| weaving (n9 WeavingMobEffect) | when its bearer is killed, a player or with `mob_griefing` on: `2 + nextInt(2)` cobwebs (`randomBetweenInclusive(2, 3)` from its own random) at up to 15 positions of `randomInCube(random, 15, pos, 1)` drawn as iterated, each air-like (`canBeReplaced`) above a face sturdy upward and not already taken, stopping at the count; each `setBlockAndUpdate` cobweb and level event 3018; particle `item_cobweb`. In another region's land a cobweb is a `SetIf` expecting what was read in the copy |
| wind_charged (n9 WindChargedMobEffect) | when its bearer is killed: a wind burst at its feet + half its height, radius `3 + nextFloat() · 2` (its own random), the wind charge's damage calculator, `ExplosionInteraction.TRIGGER` (blocks that answer TRIGGER toggled or rung, none broken; entities pushed and none hurt, as the calculator says once dumped), the gust particles and `breeze_wind_charge_burst`; particle `small_gust`. Needs `AbstractWindCharge`'s calculator and the blocks' `onExplosionHit` dumped (section 18, "what the lead must dump") |
| infested (n9 InfestedMobEffect) | at each hurt that took: own `nextFloat() <= 0.1`, then `1 + nextInt(2)` silverfish, each at its feet + half its height, turned by `randomBetween(−π/2, π/2)` from its look × 0.3 (y × 1.5) as its motion, yaw `nextFloat() · 360` of the level's random, with `silverfish_hurt`; particle `infested` |
| night vision, conduit power, dolphin's grace, luck, bad omen and the others | held and shown, no rule on the server side that N9's world reaches (bad omen needs raids, N10) |

**Refusals** (`canBeAffected`): the three tags built, and the types' own: spider and cave
spider refuse poison, the parched weakness, the wither skeleton wither, the nautilus
poison (finding 14), the undead (`#ignores_poison_and_regen`) poison and regeneration,
`#immune_to_infested` (the silverfish) infested and `#immune_to_oozing` (the slime)
oozing. A table by kind in `effects.rs`, not a hook in each type's file.

**Particles**: `DATA_EFFECT_PARTICLES` (index 10, a list of particles: each visible
effect's own particle, which is `entity_effect` with the effect's colour, ambient ones
fainter, except the four of the table above, whose particles are `item_slime`,
`item_cobweb`, `infested` and `small_gust`) and `DATA_EFFECT_AMBIENCE_ID` (index 11, whether
every effect is ambient, beside it: finding 36) sent for mobs and players, so a poisoned
golem and a player who drank a potion show their swirls (5.4). Today neither is sent
(`services/edge/src/tracking.rs` 455–466).

The every-tick work is the game's `tickEffects` (`effects.rs` 395): the check is made with
the remaining duration before the countdown (`remains(now − 1)`, M9 5).

### 5.3 Who gives effects to mobs in N9

The witch (its potions: slowness, poison, weakness, harming at a target; healing and
regeneration at a raider), the stray's, bogged's and parched's arrows (slowness 600,
poison 100, weakness 600), a spectral arrow (glowing 200), the husk's hunger (at easy
`140 · (int) eff` is usually 0 ticks, but it is still an `addEffect`, present for one
tick: finding 47, pinned in a test), the elder guardian's mining fatigue (to survival
players only: nobody), the players' potions, tipped arrows and clouds, the wandering
trader's and the witch's own drinking (built).

### 5.4 A player's effects (decision 7; the lead's)

- **Where**: the region's `Player` (`region.rs` 311–364) gains `effects: Effects` and the
  `PlayerState` (`state.rs` 58–110), `PlayerTransfer` (`api.rs` 372–408), `Place`
  (`api.rs` 89–99) and `LoggedPlace` with them. The region keeps each end in its own
  ticks; `PlayerTransfer`, `PlayerState` at a merge and the place carry the ticks
  remaining, and an arrival, a merge's survivor and a join count the ends on from their
  own tick, as `attack_reset` is carried (`region.rs` 1718; finding 1). An effect does
  not run down while its player is away, as the game's. The worker's `STATE_FORMAT` and
  the store's format rise in N9d (section 12).
- **When**: the player's tick (`LivingEntity.baseTick`'s `tickEffects`), at the players'
  part of the region's tick, before their use (5.5) and before they pick things up
  (`region.rs` 1112–1123).
- **What a creative player feels**: every hurt refused (`invulnerable`), so poison,
  wither and harming do nothing; regeneration and healing heal a full player not at all;
  hunger's exhaustion is skipped for an invulnerable player; the movement effects, night
  vision, invisibility, glowing, water breathing and fire resistance as the game's.
- **What the edge sends**: `UpdateMobEffect` (the effect's registry id, amplifier,
  duration, flags) when one is added or changed, `RemoveMobEffect` when one ends or is
  removed, to that player only, from `PlayerEvent::EffectAdded` and
  `PlayerEvent::EffectRemoved`, and **every effect again at a join** and wherever the
  edge sends a player their own state afresh (finding 36); `UpdateAttributes` of their
  own movement speed and safe fall distance when a modifier changes (the client moves by
  its attributes); to other clients, **the player's shared-flags byte whole** (on fire,
  crouching, sprinting, swimming, invisible, glowing, fall flying, as Clustine has them:
  decision 6 of section 18; today the edge sends none for players, `tracking.rs`
  461–466) and the effect particles with index 11, from a `RegionEvent::PlayerData` the
  region reports when any of them changes. The edge today sends a player's data only for
  sleeping (`fanout.rs` 6259).
- **Who gives a player effects**: a splash or cloud of their own region; one across a
  line by `EntityUse::Effects` addressed to the player (4.4, 8.3); their own eating and
  drinking (5.5); an arrow never (refused: creative). Milk clears them, honey poison, as
  the consumables' data says.

### 5.5 The use state (decision 3; the lead's)

`Using { hand, remaining: i32 }`, **with no copy of the stack** (finding 19): the game's
`updatingUsingItem` reads the hand every tick and stops the use when the item there is no
longer the same item (`isSameItem`), and the crossbow's `onUseTick` writes
`charged_projectiles` into the hand's stack, so the use reads and writes the slot itself:

- **On a mob**: a body part (parts are versioned, `mobs/part.rs` 68–91, so no format
  raise), ticked in the mob's tick before its AI (`updatingUsingItem` then `aiStep`); the
  trader's `using` (`wandering_trader.rs` ~62–76) moves onto it, its tests unchanged.
  **The witch does not** (finding 11): its `drinking` and `using_time` (`illagers.rs`
  51–75) stay its own, as the game's `DATA_USING_ITEM` (index 17) and `usingTime` are
  (n7 Witch 150–219); the living flags are never sent for it, as the official server
  never sends them.
- **On a player**: a field of `Player`, `PlayerState` and `PlayerTransfer` (a use goes on
  across a hand-over, which the client does not see), not of the place (a leave ends it,
  as the game's). Started by `PlayerInput::UseItem` where the item's `use` starts one;
  ended by `PlayerInput::ReleaseUse` (new; where it goes in the enum does not matter,
  since a world made before is refused: finding 55), which the edge makes of
  `ServerboundPlayerAction`
  `RELEASE_USE_ITEM` (6); ticked at the players' part of the tick (5.4), where the game's
  `Player.tick` ticks it: `onUseTick`, the countdown, completion at 0 unless the item
  uses on release; a change of the held slot, or the item gone from the hand, stops it.
- `startUsingItem` and `stopUsingItem` with the game events `item_interact_start` and
  `item_interact_finish` (sculk sensors hear them), and the living flags: bit 1 using,
  bit 2 the off hand, sent for mobs and players (a player's to the others: their own
  client predicts it). Every item whose `use` starts a use shows so (the shield raised,
  the spyglass, food and drink being taken).
- **Eating and drinking** (decision 5 of section 18; n9 Consumable, Consumables,
  ConsumeEffect and its five kinds, FoodProperties): `startConsuming` where `canConsume`
  (a creative player always can), the use lasting `consume_seconds · 20`; while it runs,
  `emitParticlesAndSounds` where `shouldEmitParticlesAndSounds(remaining)` holds (the
  item's crumbs, `spawnItemParticles` of the stack, 5 a time, and the eating or drinking
  sound with its `nextBoolean`, triangle and pitch draws from the user's stream: a
  player's aside, a mob's own), and at the end 16 crumbs and the sound; `onConsume`: the
  stack's `ConsumableListener`s (`potion_contents`, `suspicious_stew_effects`,
  `ominous_bottle_amplifier`, `food`, whose hunger does nothing to a creative player), then
  **every `on_consume_effects` entry of the data** in order: `apply_effects` (each a
  `probability` draw, `nextFloat() < p`, always drawn: rotten flesh's 0.8, raw chicken's
  0.3), `remove_effects` (honey: poison), `clear_all_effects` (milk),
  `teleport_randomly` (the chorus fruit: diameter 16, through the same `randomTeleport`
  as the enderman's, S9's) and `play_sound` (the ominous bottle); then the
  `item_interact_finish` game event, and nothing consumed in creative. The draws are the
  user's in the game's order. A sound and crumbs with no `has_consume_particles` are as
  the data says (drinks have none).

---------------------------------------------------------------------------------------

## 6. Mobs steered by their riders (decision 5; the lead's)

The game: the first passenger, a `Mob` that can control a vehicle that is not `NoAI`,
steers through the vehicle's navigation and move control (`Mob.getNavigation`,
`getMoveControl`), the vehicle's own move, look and jump goals stopped by
`updateControlFlags` (M9 6). The vehicle moves first in the tick by what its rider wrote
the tick before; the rider then writes again.

- **The second view.** `tick_passengers_of` (`region/riding.rs` 805–854) reads the
  vehicle's record before the rider's tick. Where the rider controls it, the rider's
  `MobTick` gains `controlled: Some(Controlled { key, kind, mob_type, mob, living })`:
  the vehicle's decoded record. The rider's `Ai` gives every navigation and move-control
  call the vehicle's (`navigation.rs` 305–545 and `controls.rs` 25–60 take the record
  they act on, which is the rider's own or `controlled`'s), and its look and jump
  control stay the rider's. The path is planned with the vehicle's position, size, node
  evaluator, maluses and follow range; the speed is the rider's modifier times the
  vehicle's. After the rider's tick, **the vehicle's whole navigation state** is written
  back into its record by `commit_entity`, within the region's tick, before
  `position_rider` (finding 39): the path and its progress, `speedModifier`, the stuck
  tick and position, the timeout, the recompute time and the tick counter, the move
  control's operation, wanted position, speed and strafe, the jump control, and (for the
  bow goal's `controlledVehicle.lookAt`) its rotation. Nothing crosses a line: a unit is
  in one chunk and one region (`riding.rs` 10–17).
- **The flags.** `update_control_flags` (`ai.rs` 754–765) is fed the truth every fifth
  tick: MOVE and LOOK off where a mob controls the vehicle, JUMP off for that or a boat.
  `MobTick.vehicle` gives the vehicle's kind today; the tick gains `controlled_by_mob` for
  the vehicle's own tick.
- **The stroll** (`goals.rs` 351–352) gains `hasControllingPassenger`.
- **`rideTick`**: the skeleton's and the drowned's copy the vehicle's body yaw where it is
  a `PathfinderMob`.
- **The reach**: `getAttackBoundingBox` widened in x and z over the vehicle's box
  (Mob.txt 3262–3361), so a jockey's rider reaches as its mount is wide.
- **A brain vehicle** (the zombie nautilus) keeps driving its own navigation from its
  brain, and its drowned writes it too, as the game's (M9 6, **Inferred**); the order in
  the tick decides which wins, as in the game.
- **What it gives**: the chicken jockey chases what its baby zombie targets; the spider
  jockey's skeleton paths and strafes its spider by the spider's wall-climbing
  navigation; a zombie nautilus goes where its drowned wants. P7k 11 item 1's named
  difference goes.

---------------------------------------------------------------------------------------

## 7. The difficulty (decision 8)

- `Difficulty::WORLD` stays easy (`clustine-world/src/difficulty.rs`); every rule reads it
  and its id: the uncertainty `14 − 4 · id` (10), the skeleton's interval (40; the bogged
  and the parched 70), the guardian's damage, the cave spider's poison (none), the
  reinforcements (none), the doors (none), the conversion (none, the villager dies),
  peaceful's refusals (never).
- **The effective difficulty** (`DifficultyInstance`, M9 7): `eff = 1 · (0.75 + t + c')`
  with `t = clamp((clock − 72000) / 1440000, 0, 1) · 0.25`, `c = clamp(moon · 0.25, 0, t)`
  (inhabited time 0), `c' = c · 0.5`; the clock is the overworld's day clock (the region's
  `clock.day`), the moon's brightness `making::moon_brightness(day)` (`making.rs` 75–78,
  the phase table D9 5.9 confirms). At easy `eff` lies in [0.75, 1.125]: `(int) eff`
  reaches 1 only where `t + c' ≥ 0.25`, which needs `1.5 · t ≥ 0.25` at the brightest
  moon, so a clock of at least 72 000 + 960 000 = **1 032 000 ticks** (about 14.3 hours:
  finding 41 corrected the draft's 960 000), and then on days of a bright moon, **all
  day**, not only at night (the moon's brightness is the day's phase), when a burning
  zombie's hit sets fire for 2 s and a husk's gives 140 ticks of hunger.
  The special multiplier is 0. One function, `difficulty_at(pos)`, used by every
  `finalizeSpawn` the region makes (eggs, natural spawning, the evoker's vexes, the
  patrols, the phantoms) and the rules of section 8 that read it.
- **Generation** keeps `(0.75, 0)` at day 0 and a full moon, which is what a world's first
  day gives: a chunk generated later in the game would read the clock then, which a
  function of the chunk (A21) cannot. At easy the effective difficulty is read by no
  `finalizeSpawn` that generation reaches. Named.
- **Inhabited time** is not kept (named, decision 8).

---------------------------------------------------------------------------------------
## 8. Monsters

### 8.1 Targets (M9 1; the lead's first mob commit, decision 9)

- **Every placeholder replaced at once**: `TargetOfN9` (`monster_ai.rs` 67–105, in the
  creeper, spider and skeletons) and `tamable::Idle` as a target (`zombie_family.rs`
  254, `drowned.rs` 389–391, `illagers.rs` 147–157 for the witch, vindicator, pillager and
  evoker, `guardians.rs` 190–196) become the game's goals of M9 1.2's table, by priority,
  with their selectors: `BABY_ON_LAND_SELECTOR` (baby turtles on land), the drowned's
  `okTarget`, the spiders' light test before the interval draw, the witch's
  `NearestHealableRaiderTargetGoal` (`nextBoolean()` every asked tick, then no raid) and
  `NearestAttackableWitchTargetGoal`, the illagers' `canAttack` (not a baby villager), the
  vindicator's Johnny goal (no draw), the evoker's 300 unseen ticks, the vex's
  `VexCopyOwnerTargetGoal`, the guardians' `GuardianAttackSelector`, the slime's height
  test, the enderman's `EndermanLookForPlayerGoal` (finds no creative player) and its
  endermite target, the silverfish's. Selectors that read the world (`fn(&Looked) ->
  bool` today, `goals.rs` 1474–1544) take the `Ai` as `tamable::nearest_target` does.
- **Draws**: one own `nextInt(5)` per asked selector tick before any search (M9 1.1) **in
  the goals that are `NearestAttackableTargetGoal`s with an interval**, where the
  placeholders of five types drew nothing (M9 0.4). **Not every target goal draws**
  (finding 29): `EndermanLookForPlayerGoal` (n9 Enderman$EndermanLookForPlayerGoal: no
  draw), the phantom's (a countdown), `VexCopyOwnerTargetGoal` and `HurtByTargetGoal`
  draw nothing, and the witch's two draw only where they are asked, as their dumps
  have it; a table in N9f says, goal by goal, what each draws. The tests that pinned
  those streams (the zombie family's, the drowned's, the illagers', the witch's, the
  guardians') change in the same commit, each said in its message.
- **`alertOthers`** (M9 1.1, `HurtByTargetGoal`, `goals.rs` 1403–1470): in `start`, every
  mob of the attacker's class (by the game's `instanceof`: a zombie alerts zombies,
  husks, zombie villagers and drowned; a husk only husks; a drowned drowned; a raider
  raiders of its class) in `unitCube(position).inflate(follow, 10, follow)` with no
  target, not an ally, not another owner's, not of a class in `toIgnoreAlert` (matched by
  **exact class**, `getClass() ==`, not by identity: finding 28), in the region's order,
  is given the attacker as its target. Those in copies get `EntityUse::Alert { attacker }`
  (8.3), which their holder applies by the same tests. The box reaches the follow range
  (35 for a zombie) in x and z, past the copies' reach where a line is near: a mob beyond
  the copies is not alerted (named).
- **The difficulty's reads** (section 7) and the creative locks (M9 0.1, built).

### 8.2 The hit (`Mob.doHurtTarget`, M9 0.8)

`melee_hit` (`goals.rs` 1625) becomes the game's: the attack damage, the weapon's damage
source and `modifyDamage` (Sharpness and the item's own bonus, from the data), the hurt,
and only if it took: `causeExtraKnockback` with `attack_knockback`, `hurtEnemy` (wear),
`doPostAttackEffects`, `setLastHurtMob`, the attack sound. A type's hit
(`MobType::do_hurt_target`, `types.rs` 298) adds its own after the parent's: the zombie's
fire (`nextFloat() < eff · 0.3` when it took, empty-handed and burning;
`igniteForSeconds(2 · (int) eff)`), the husk's hunger (`140 · (int) eff` ticks), the iron
golem's fling (built), the guardian's two hurts, the vex's charge.

**Where and when the extras run** (finding 5; decision 1 of section 18; decision 18 as
now written). Even inside one region a mob's hit is a deferred `Local::Hurt`, landed by
`hit_by_mob` (`brains.rs` 87 on) after the attacker's tick (`goals.rs` 1625–1636,
`mobs/tick.rs` 100–117), so the extras that hang on the outcome cannot run in the
attacker's tick. They run **where and when the hit lands**, the same way in one region and
across a line: the hurt carries the attacker's key, kind, its `lived` at the hit, its
weapon and what its type adds (`Local::Hurt` and `EntityUse::Hurt` alike), and the
lander runs the hurt, then the extras by its outcome, drawing from **the hit's stream**,
`Random::of_hit(world, attacker, attacker's lived, target)`, a new stream of
`region/random_ticks.rs` with a mark of its own beside `of_entity_aside` (keyed by the
target too, so that a piercing arrow's hits draw apart; the region's tick is not in it,
since the lander's region differs). Wear (`hurtEnemy`) writes the attacker's weapon: in
one region the lander writes it into the attacker's record; across a line the wear is
named lost (a mob's sword wears not at all from a hit across a line, which nobody sees:
no mob's weapon breaks at easy within a play). `setLastHurtMob` for a melee hit is set by
the lander likewise. **Named against the game's order of draws**: the game draws these
from the attacker's own random within its tick; Clustine from the hit's stream when it
lands, so the attacker's own stream no longer holds them.

**The swing** is `MeleeAttackGoal`'s (`swingForAttack(MAIN_HAND)`, then `doHurtTarget`:
finding 45), not `doHurtTarget`'s, so the vex, the guardian and the slime, which hit
without that goal, do not swing. It is the item's attack animation, `LivingEntity.swing`
and `swingState.startIfAble`, which today only players have (`region.rs` 2559–2585): a
mob gains the swing state (a body value, memory only, as the player's) and its
`RegionEvent::ArmSwung`.

### 8.3 What crosses a line: the entity effects (the lead's)

A28 section 7's entity effects, carried and taken as now (`send_on_entity`,
`breeding.rs` 275–288; `entity_used`, `entities.rs` 231). New uses, each a new tag of the
effects' format (12 to 16; the last is `USE_MOB_HIT_THORNS` 11, `clustine-format/src/
effects.rs` 57–71), added in N9e (a new tag needs no format raise; the draft's "so that
the log's older records read" was no reason, since worlds made before are refused:
finding 55):

| Use | Carries | The holder |
|---|---|---|
| `Hurt` (12) | the damage source (its type by name; causer key, kind and position; the direct entity's key and kind; the projectile's motion), the amount, the knockback, and **what follows if it took**: fire to set first and restore on a refusal, fire to set after, effects (instances, already scaled), an arrow to count, the zombie's fire chance and the husk's hunger as the attacker's type gives them, a hit sound, and what the hit's stream needs (the attacker's key and `lived` at the hit, 8.2) | runs the hurt by its own rules (`hurt_by`, `body.rs` 1105), then the extras by its outcome, drawing them from the hit's stream as a lander in the attacker's region would. Mobs' hits, arrows', tridents', fangs', rockets', the guardian's beam, a splash's water hurt, the vex's charge all use it; `MobHit` stays for what writes it today |
| `Alert` (13) | the attacker's key, kind and the class to match | gives the target if the mob has none and passes the tests of 8.1 |
| `Effects` (14) | the instances (scaled), the instant ones with their factor, the source (a potion's or a cloud's key, its kind, and its owner) | `addEffect` each (the mob's `canBeAffected`; a player's store); instants applied with the factor and `indirectMagic(source, owner)`; an enderman hurt only if the potion is water and it is water-sensitive (finding 25) |
| `PearlLanded` (15) | the position, whether an endermite comes | the player with that entity key: the teleport and the rest of 4.4 |
| `PatrolTarget` (16) | a patrol target (a block position) | a patrolling mob that `canJoinPatrol` takes it as its patrol target (the leader's `LongDistancePatrolGoal`, n9 PatrollingMonster$LongDistancePatrolGoal, sets it on companions within 16 blocks, which may be in a copy: finding 26) |

**Addressed to players** (finding 27; question 6 as the review answers it). A player's
region and the land they stand in can differ (a face of another region's player in held
land is ordinary), and `send_on_entity` (`breeding.rs` 275–286) resolves only entities
of copies, `entity_used` (`entities.rs` 231–297) only chunk entities. So `Hurt`,
`Effects` and `PearlLanded` addressed to a player go by a path of their own: the effect
names the player (`Effected::OnPlayer { player, entity, by }`), carried with the commit as
every effect is, and **the store delivers it to the region that holds that player's
stay** when it takes the group (it knows each stay's holder, ADR-0020), not to the
holder of a chunk; a player whose stay is between regions at that moment misses it
(named: a hand-over takes a few ticks). The holder looks the player up by entity key and
applies it. The sender knows a player by their face (its entity key) or by the players'
table (the pearl's owner beyond the faces). This is the lead's (N9e), and it is the one
new path through the store N9 makes; A28 section 7 records it.

*As built (N9e)*: the effect is addressed to the block where the sender last saw the
player, and the holder of that chunk takes it for the player of that entity, where it has
them; the store routes it as any effect. The store keeps no stay's holder, and the two
differ only while a hand-over is under way, which both miss. ADR-0028 section 7 records
it.

**Interactions at the line.** Each of these, being `Effected::OnEntity`, already counts as
work at the line for the merge of regions that follow players (`arrivals.rs` 95–115);
`interactions_at_line` gains `Effected::OnPlayer`.

### 8.4 Each monster (M9 2 and 3; D9 3)

The game's goals, as M9 has them, by type. Where a goal is shared by types of more than
one part, it is the lead's, made real in N9f in `mobs/ai/goals.rs` (one file, as now:
the draft's `mobs/ai/goals/` does not exist): `MeleeAttackGoal` (with the swing, 8.2),
`RangedAttackGoal` and `Raider$HoldGroundAttackGoal`; a goal of one part's types only is
in a file of that part's (section 12).

- **The zombie family** (zombie, husk, zombie villager; M9 2.2): `ZombieAttackGoal`
  (`MeleeAttackGoal` with `raiseArmTicks`, aggressive from 5 with the attack under 10:
  the client's raised arms), the turtle-egg goal (built), `MoveThroughVillageGoal` (finds
  no village until N10: `canUse` false after its checks, its draws as the game's), the
  stroll, the looks; `SpearUseGoal` registered and running only with a kinetic weapon
  (one zombie in 600, a camel husk's husk in ten: built with the spear in 8.6). Hard-only
  rules (doors, reinforcements, conversion) are not reached and draw nothing at easy.
- **The drowned** (M9 2.2, 3.7): its trident goal (`RangedAttackGoal` with a trident in
  the main hand, raising it by `startUsingItem`, never released), `DrownedAttackGoal`, its
  own swimming goals (built), `okTarget`.
- **Skeletons, strays, bogged, parched** (M9 3.2): `reassessWeaponGoal` (bow goal or
  melee goal at 4, on spawn, load and equipment change), `RangedBowAttackGoal` with its
  strafing and its two always-drawn floats, `performRangedAttack` (`getMobArrow`: the
  triangle, the aim at `getY(1/3)`, 1.6 and 10, the sound's draw), the variants' arrow
  effects (`Arrow.addEffect`, so their arrows are coloured); `AvoidEntityGoal` of wolves
  (built), so they flee wolves and shoot none (finding 46). Their drops' loot functions
  that now have a component to write, `set_potion` (the stray's, bogged's and parched's
  tipped arrows) and the enchanting functions, are applied (finding 56;
  `mobs/body/loot.rs`).
- **The creeper** (M9 2.4): `MeleeAttackGoal(1.0, false)` towards what hurt it; swelling
  and the explosion built.
- **Spiders** (M9 2.3): `SpiderAttackGoal` (not as a vehicle; drops its target in light
  with `nextInt(100)`), `LeapAtTargetGoal(0.4)` (built), the cave spider's poison not at
  easy.
- **The witch** (M9 3.4): `RangedAttackGoal(1.0, 60, 10)`, `performRangedAttack` (the
  potion by the target's state, the weakness draw only where it is reached, the splash
  potion shot at 0.75 or 0.45 and 8.0, the throw sound's draw unless silent), swiftness
  when its target is beyond 11 (`witch.rs` 112's `target_far` made real), its 0.15
  against `#witch_resistant_to` (built).
- **The pillager** (M9 3.3, D9 3.4–3.5): `RangedCrossbowAttackGoal` (no `start`; `stop`
  sets the target none), the charge by the use state (25 use ticks, `releaseUsingItem`'s
  extra use tick loading it and drawing the end sound's level float before the goal's
  `nextInt(20)`), `performCrossbowAttack` (1.6, 10, no crit, `crossbow_hit`), the
  charging flag (data 17) and its arm pose; `HoldGroundAttackGoal` for patrols (8.6).
- **The vindicator** (M9 2.5): `MeleeAttackGoal(1.0, false)`, `HoldGroundAttackGoal`, the
  break-door goal that needs a raid (no draw).
- **The evoker** (M9 3.5): `SpellcasterCastingSpellGoal`; the fangs (40, every 100: a
  ring within 3 blocks, else a line of 16, `createSpellEntity`'s floor search, 4.8), the
  vexes (100, every 340: `nextInt(8) + 1 > vexes within 16`, three vexes with their
  `finalizeSpawn` and `setLimitedLife(20 · (30 + nextInt(90)))`, made across a line by
  4.1's rule), the wololo (no target needed, but `mob_griefing` on, which it is: finding
  48; blue sheep within 16 that it sees, `nextInt(size)`, red); the spell byte (data 17)
  and the cast sounds.
- **The vex** (new, n7 Vex; M9 2.7): no physics during its tick, no gravity, its move
  control, its charge (data 16, bit 1, as D9 counts it), its random move, its limited life
  and the `starve` hurt; it never burns; an iron sword at spawn.
- **The guardians** (M9 3.6): `GuardianAttackGoal` (the beam's target in data 17 as an
  entity id, entity event 21 at `attackTime == 0`, the strike at 80, the elder's at 60:
  `indirectMagic` then its melee), the body's turn to its head while attacking.
- **The slime** (n9 Slime, AbstractCubeMob; D9 5.5): `AgeableMob` (baby and age-lock at
  16 and 17, the size at 18), its size (health `n²`, speed, attack `n`), its move control
  and jumps, its four goals and two targets, its split into `2 + nextInt(3)` smaller ones
  at death, `finalizeSpawn`'s size draws; a frog can now eat a small slime (the frog's
  tongue finds prey, which P8 and the roadmap named; `frog.rs` and `frog_ai.rs` are S9's
  for N9).
- **The enderman** (n9 Enderman; D9 5.6): its goals and targets (the stare test finds no
  creative player), the daylight teleport (`customServerAiStep`), the water hurt, the
  projectile refusal (up to 64 teleports, the arrow flying on), the carried block (data
  16) with `EndermanTakeBlockGoal` (`nextInt(10)`, three `nextDouble` offsets, a clip,
  `#enderman_holdable`) and `EndermanLeaveBlockGoal` (`nextInt(1000)`), the creepy and
  stared-at flags (17, 18). Its refusals as the game's (finding 25): it refuses a
  projectile only when it is no passenger; a thrown potion hurts it only if it is water
  and it is water-sensitive (a harming splash does nothing to it); after a hurt whose cause
  is no living entity it draws `nextInt(10)` and teleports unless it is 0, nine times in
  ten (n9 Enderman 885–945). **Across a
  line**: a block taken from a copy is a `SetIf` expecting the block, a block put down
  there is a `SetIf` expecting air; the enderman's own change of carried state is made
  when the take is sent, as the snow golem's trail is decided on its copy (P7k 10 "At a
  line"). **Named, both** (question 7): a take refused at the holder (the block changed
  in that tick) leaves the enderman holding a block the world still has, a duplicate;
  a put-down refused (the place filled in that tick) loses the block. Teleports are
  moves (a teleport into another region's land is a crossing, as any); `randomTeleport`
  reaches 32 blocks, past the copies, and a destination where the region can read
  nothing fails, as the game's fails at an unloaded one (named). `randomTeleport` is one
  function (`region/teleports.rs`), which the chorus fruit (5.5) calls too.
- **The phantom** (n9 Phantom and its inner classes): its size (data 16), its move,
  look and body controls, its circle around an anchor, its attack strategy (finds no
  creative player: it circles), its burning in daylight. Made only by its spawner (9.7)
  and its egg.
- **The endermite** (n9 Endermite, dumped after the review): its goals, its two-minute
  life, its spawn rule (from a pearl only, at easy: monsters spawn).
- **The silverfish** (n9 Silverfish, `SilverfishWakeUpFriendsGoal`,
  `SilverfishMergeWithStoneGoal`; decision 2 of section 18): its goals and targets, its
  merging into a stone block (an infested block, with `mob_griefing`), its waking of
  friends in infested blocks around when hurt (breaking them, `spawnAfterBreak` making
  more), and infested blocks broken by an explosion making one (`region/infested.rs`;
  `InfestedBlock` owed). Its block writes in another region's land are `SetIf`s, as the
  enderman's.

### 8.5 The merge for targets across a line (decision 17; the lead's)

A28 section 7: where regions follow players, they merge at once when an entity "targets or
is targeted across" their line. Built in the hook that notes work at a line
(`arrivals.rs` 119–130, `at_line`): after the entities tick, every mob whose target
resolved in a copy (`Looked.copied`) or to a face of a player outside the region's land
notes the chunks within four blocks of itself and of its target, for 600 ticks, as an
arrival does, **both sets noted against the target's holder** (finding 40: `at_line`
takes its "to" region from the holder of the chunk it is given, `arrivals.rs` 119–130,
so the chunks about the mob's own position would name the region itself; the hook gains
a form that takes the "to" region apart from the chunks). The worker reports them with
the rest (`redstone_at_work`, `services/worker/src/lib.rs` 3214–3254) and the
coordinator merges the pair at once
(`policy.rs` 365–391). Pinned lines keep the entity effects of 8.3. Tested with a horde of
zombies chasing villagers over a line, both ways, where the regions follow players
(section 14).

### 8.6 Patrols and the spear

- **`PatrollingMonster`** (n7 PatrollingMonster): the patrol goal (the leader walks to a
  patrol target, the others follow, `findPatrolTarget`), the leader flag, the leader's
  ominous banner on its head with drop chance 2.0 (the components of section 3), `canJoin`
  and `removeWhenFarAway` (built for the raiders), and `LongDistancePatrolGoal` (n9
  PatrollingMonster$LongDistancePatrolGoal, dumped after the review: the leader near its
  target finds a new one, `findPatrolTarget` ±500, and sets it on its companions within
  16, those in copies by `EntityUse::PatrolTarget`, 8.3; its `moveRandomly` draws two
  `nextInt(16)`; a 200-tick cooldown on a failed path). `Raider$HoldGroundAttackGoal` (M9
  2.5) is the lead's (N9f). The pillager, vindicator and evoker register them (today five
  placeholders, `illagers.rs` 138–144, which N9f splits by type); the raid goals stay
  placeholders that never start (no raids).
- **`SpearUseGoal`** (390 lines, n7, dumped; `KineticWeapon` and its `Condition`, n9,
  dumped after the review): with a kinetic weapon in hand. The review (question 9) asks
  that the mob's spear and the player's be decided together: `SpearUseGoal` runs the use
  state and `KineticWeapon`'s damage by speed, and the edge drops the player's `STAB`
  today. Left to the lead (section 18, open questions); until decided, S6 builds the
  mob's use if it is decided in, and otherwise the holder falls through to
  `ZombieAttackGoal`, named.

---------------------------------------------------------------------------------------

## 9. Natural spawning

### 9.1 Where it runs, and its switches

In the region's tick, where the game's `chunkSource.tick` is (n7 ServerChunkCache 794–961):
after the block and fluid ticks and post-processing, **spawning, then the random ticks**
(`run_random_ticks`, `region.rs` 1104), **then the custom spawners**; then block events
and entities as now. The game rules `spawn_mobs` and `spawn_monsters` are on (Clustine has
no game rules); `spawnEnemies` is `isSpawningMonsters` and never reads the difficulty
(D9 5.4). Creatures (the persistent category) only when `gameTime % 400 == 0`. **The
categories are filtered by the global cap once, at the start of the phase** (finding 2:
the game calls `getFilteredSpawningCategories` once, n7 ServerChunkCache offset 125,
before it shuffles the chunks at 180 and spawns at each, and `spawnForChunk` then asks only
the local cap), so a category open at the start stays open for the whole phase whatever
the region spawns in it.

### 9.2 The caps (decision 12; the lead's; revised by findings 2, 4 and 30)

What the game computes once a tick from the level, Clustine computes from what the region
has now and what the store last published of the other regions:

- **What each region reports.** For each chunk it has loaded (its own, not copies), the
  number of its mobs by category (seven, MISC never), counting what the game's
  `createState` counts: not a persistent mob (`isPersistenceRequired` or
  `requiresCustomPersistence`: a passenger, a leashed mob, an enderman carrying a block),
  not MISC. A tick's output carries the counts of each chunk whose counts changed, whole,
  and a chunk let go as nothing; a region's first tick after a hello, a restore, a merge
  or a split carries a **full report**, every loaded chunk's. They travel with the commit
  but are **not written to the log** (question 12).
- **What the store keeps** (finding 4). The counts **by (region, chunk)**, in memory, read
  when the group is durable: a partial report changes that region's entries, a full
  report replaces the region's whole set, and a region's set is dropped when it loses its
  owner (a take-over) or is absorbed (a merge; a split's parent likewise), so a lend's or
  a return's "nothing" arriving after the borrower's report zeroes only the lender's
  entry, a take-over that loads other chunks leaves nothing of the old owner's counted,
  and merges and splits cannot race. After a store restart the store publishes **no
  counts until every owned region has sent its full report** (every region's hello
  sends one), so spawning pauses for about a second there (named).
- **What the store publishes** with the clock and the world's players, once a second
  (`StoreReply::Clock`, `lanes.rs` 2178–2193; `CLOCK_EVERY` 20), **to each region apart**:
  the totals by category over **the other regions'** chunks; for each `WorldPlayer`, their
  local counts over **the other regions'** chunks (the sums over the chunks whose centre
  lies within 128 blocks of them horizontally, `(cx·16 + 8 − px)² + (cz·16 + 8 − pz)² <
  16384`, D9 5.1), non-spectators only, **reckoned against the positions it publishes in
  the same message** (finding 30). Seven numbers a player and seven totals, a region:
  with ten regions and 100 players about 7,000 numbers a second, small (measured,
  section 15).
- **The region adds its own, as they are now.** The finding's concern was a region's own
  changes mixed with published counts of its own chunks reckoned at another time and
  position; leaving the region's own chunks out of what it is published removes it. So
  the region counts its own chunks now (its loaded mobs by category, kept as a running
  tally) and, per player, its own chunks within 128 blocks of the position it uses for
  that player now; nothing of its own is counted twice or a second late. A chunk that has
  just changed hands may be counted by both the old holder's set (until its report drops
  it) and the new holder's own tally for about a second: the caps err towards spawning
  less (named). *This takes finding 30 in a simpler form than the review's correction
  (its own count now near the position it uses, less its own count at the counted tick
  near the published position), which needed the region to keep its counts as of the
  store's counted tick; nothing is lost by it.*
- **The spawnable chunk count** each region computes from the players' table: the chunks
  within Chebyshev 8 of each tracked player's chunk, each once, loaded or not (D9 5.2);
  spectators are tracked (`spectators_generate_chunks` is on by default).
- **The global cap**, once at the start of the spawning phase (9.1; finding 2):
  `others[c] + own[c] < max[c] · spawnable / 289`. Spawns in the phase do not close it
  again within the tick, as the game's do not.
- **The local cap**, per chunk and category, before the position's draws (D9 5.1): the
  players near the chunk, from the table (others) and the region's own (live positions),
  fixed for the tick at the chunk's first use; it passes if any of them has fewer than
  `max[c]` counted, their count being the published others' plus the region's own now in
  chunks near them; a player with nothing counted passes at once; no player near, no
  spawn. The region's `afterSpawn` raises its own counts within the tick, which this cap
  reads.
- **How late**: another region's spawns and deaths count about a second late (the
  publication's period and a commit's latency), the region's own at once. Two regions
  near the global cap may each take the last places in the same second, as a tick of the
  game's may overshoot (M9 0.24); the overshoot is bounded by one second of the other
  regions' spawning. Named.
- **Before the first publication** a region spawns nothing (and despawns nothing, 9.5).

### 9.3 One chunk (M9 8.3; D9 5.8)

- **The spawning chunks**: chunks in which the region's entities tick (A28 section 2),
  whose centre is within 128 blocks horizontally of a non-spectator player (own, face or
  table), within Chebyshev 8 of a tracked player's chunk.
- **Their order**: the game shuffles all of a level's spawning chunks with the level's
  random. Each region orders its own by a key drawn from each chunk's stream for the
  tick, which is a uniform shuffle in distribution; the draws are not the game's `size −
  1` (named: draws are the game's in distribution, not in value, as A28 section 4 has it).
  Across regions the order of chunks matters only to who takes the last places under a
  cap.
- **Each chunk**, from its stream with a purpose mark of its own (A28 section 4: spawning
  draws from its chunk's stream), in the game's order (n7 NaturalSpawner
  `spawnCategoryForPosition`, offsets 0–485): each category still open (9.1) in enum
  order behind the local cap; `getRandomPosWithin` (x, z, y up to the surface heightmap +
  1); a redstone conductor there ends it; three groups, each with a pack size
  `ceil(nextFloat() · 4)`, each attempt stepping `nextInt(6) − nextInt(6)` in x then z;
  the nearest non-spectator player at any distance (own, face, table), none → next
  attempt; more than 24 blocks from them and from the world's spawn point; the step's
  chunk must be one the region's entities tick in (**another region's land is refused**,
  decision 13); the type once per group (`getRandomSpawnMobAt`: the structure's override
  or the biome's list, M9 8.3; WATER_AMBIENT's 0.98 in `reduced_water_ambient_spawns`),
  none ends the group, **and at that pick the pack size is drawn again and replaced:
  `spawnerData.count().sample(random)`** (offsets 297–309; finding 3), which draws for
  a uniform count (the enderman's 1 to 4, the sulfur cube's 2 to 4) before the position
  ends; then `isValidSpawnPostitionForType` (the despawn distance, the list still having
  the entry, the placement, **the type's rule with its draws**, `noCollision` against
  blocks, the border and hard-collision entities only, D9 5.8) **for the type picked,
  whether Clustine builds it or not** (finding 31: a failure there is the next attempt,
  so the sulfur cube's placement, size for its box and its spawn rule are written, though
  the cube is not); the spawn potential's charge (no overworld biome has spawn costs:
  passes); the type made (**a type Clustine does not build ends the whole position**, as
  the game's null does: the sulfur cube, decision 11); the yaw `nextFloat() · 360`;
  `isValidPositionForMob` (the despawn distance again, `checkSpawnRules`: a monster's
  walk target value; `checkSpawnObstruction`); `finalizeSpawn` with `difficulty_at` and
  the group data (the level's draws: the chunk's stream); added with its passengers;
  `afterSpawn`; then **the type's `getMaxSpawnClusterSize`** reached ends the position
  (fish 8, schooling fish their school size, horses 6, the wolf 8, the pillager 1, every
  other 4: section 0) and `isMaxGroupSizeReached` (the tropical fish's) ends the group.
- **When a mob takes a key** (finding 32). In the game a mob's UUID and its
  constructor's draws come from its own random, apart from the level's, so where they
  fall does not change the chunk's stream. Clustine makes the mob as a type, position
  and yaw for the tests, and takes its key, with its own stream's draws (UUID and the
  rest), **only once it has passed `isValidPositionForMob`**, before `finalizeSpawn`:
  an attempt refused there takes no key and commits nothing.
- **Unknown light refuses** (finding 43): where `light_at` returns `None` (a chunk not
  yet lit, `region/light.rs` 365–403 as cited at 5a5ea15), a rule that reads light fails,
  so the attempt goes on to the next as a failed rule's would; named, and only for the
  few ticks before N8's step lights a newly loaded chunk.
- **The nearest player** is asked at every attempt; the region indexes its own players,
  the faces and the table's players by chunk once a tick, so an attempt looks at the
  chunks outward from its position rather than at every player (finding 33).
- **The runtime's view**: the region's own `SpawnView` over its loaded chunks, light
  (N8's darkened `max_local_raw_brightness`, `region/light.rs` 264–272, and the raw sky
  light), heightmaps, biomes and the kept structures (9.6). **The biome at a block is
  the game's** (finding 34; decision 3 of section 18): `ServerLevel.getBiome`, the zoomed
  lookup of `BiomeManager` with the hashed game seed, for the spawn list and for every
  rule's tag, ported in `clustine-terrain/src/biome/zoom.rs`, which the simulation already builds
  with (through `clustine-features`) and calls by a direct dependency, N9h's; the
  eight cells it looks at may lie in a neighbouring chunk, which the region has in its
  land or copies wherever it spawns. `region/animals.rs` 7–10's named difference (a
  variant's biome by its own cell) goes with it. Generation's helpers that fit are shared
  (`generation/spawning.rs`: `pick`, `is_spawn_position_ok`, `is_valid_empty_spawn_block`,
  `contains_any_liquid`, `biome_in_tag`, `magic_value`); those that do not (generation's
  `no_collision`, `check_spawn_rules` for creatures only and without draws,
  `raw_brightness` undarkened, `made_mob` with day 0) get runtime versions. The making is
  `spawn_mob_with`'s (`region/spawn_eggs.rs` 158–318), freed of its player: the draws
  come from the caller's stream, the chunk's here.

### 9.4 The types' rules (D9 5.7, every predicate named)

Each predicate of D9 5.7's table that a spawned type uses, with its draws on the chunk's
stream in the game's order: `Monster.checkMonsterSpawnRules` (`nextInt(32)` against raw
sky light always, then the block light limit, then the darkened light against `nextInt(8)`),
the surface monsters' `canSeeSky`, the stray's powder snow, the drowned's (its draws
whatever the light said), the guardian's `nextInt(20)` first, the slime's (D9 5.5: one
level float against `surface_slime_spawn_chance`, half the moon's brightness from the
`moon` timeline, in `#allows_surface_slime_spawns` between 50 and 70, then the darkened
light against `nextInt(8)`; failing that, the slime-chunk test, its `nextInt(10)` always
drawn; `seedSlimeChunk` with **the world's game seed** and the `int` overflows as
written: every world has one now, the flat world included, whose plains carry slimes
×100 below y 40, so comparison 11's flat world needs the same seed on both servers:
finding 22), the
pillager's `PatrollingMonster` rule, the animals', the water creatures', the axolotl's,
the bat's (no Halloween: P7k 10), the glow squid's, the ocelot's and parrot's. Each a
function in one file (`spawning/rules.rs`), dispatched by D9 5.7's index.

### 9.5 Despawning (decision 14)

`check_despawn` (`mobs/body.rs` 505–553) reads the nearest non-spectator player from the
region's players, the faces in its land and copies, **and the world's players' table**
beyond them; until a table has come, nothing despawns. The rest is built.

### 9.6 Structures in the chunk (decision 15)

The least that serves: each chunk carries, for every start of a structure with spawn
overrides (in the overworld: the swamp hut, the monument, the pillager outpost, the ancient
city, the trial chambers; the fortress later, with the Nether) whose bounding box reaches
the chunk, the structure's id, the start's box (for a `full` override) and the boxes of its
pieces that reach the chunk (for a `piece` override). `mobsAt` asks each in order: the
first whose override's box holds the position gives the list (an empty list for the
ancient city and the chambers: nothing spawns). Generation has the starts at the
structures' stage (`services/worldgen/src/stages.rs` 51–81) and writes them with the
chunk; the chunk's file gains a field (A22 amended: it said structures are not carried,
`docs/adr/0022-what-a-chunk-carries.md` 190–193); a chunk made before has none, but worlds
made before are refused anyway; the format rises for it in N9h, the step that adds the
field (section 12). Copies carry it as part of the chunk. **The village**: the same field
holds villages' starts (their pieces' boxes), read only by the patrol spawner until N10
(question 8, option (b)): the game asks `isCloseToVillage(pos, 2)`, a village within two
sections (finding 44), so the stand-in reads the field of the chunks within 2 of the
player's chunk and asks whether a village piece's box **widened by 32** holds the
position.

### 9.7 The custom spawners (decision 16)

The game's spawners keep a world's countdown and draw from the level's random over the
level's players. Clustine's regions share the world clock and the players' table, so:

- **The schedule** of each spawner is a function of **game time** (not of the day
  clock, which sleep moves: finding 24): its firings form a chain from game time 0, each
  next one drawn from a stream keyed by the world's number, the spawner and the last
  firing's game time (the phantoms' `(60 + nextInt(60)) · 20`, the patrols' `12 000 +
  nextInt(1200)`), so every region computes the same firings without state (the game's
  `nextTick` is not saved either); the region computes the chain once and keeps its end
  in memory. A region fires at **its first tick at or after a due time**: it fires each
  due time that lies after the game time of its previous tick and at or before its own,
  so the clock's forward jump at a publication (finding 1) skips no firing. A region
  that is new (a restore, a merge's survivor, a split's halves) counts from its first
  tick: a firing due in the hand-over's tick may be missed or made twice (named, rare).
- **Phantoms** (D9 5.4): at each firing, by night (sky darken 5 or more), each region
  rolls for **its own** non-spectator players (the game rolls for every player of the
  level, each independently: the same thing, divided): above sea level under open sky,
  `difficulty.isHarderThan(nextFloat() · 3)`, `nextInt(timeSinceRest) >= 72000`, the
  position 20 to 34 blocks up and within ±10, `isValidEmptySpawnBlock`, `1 + nextInt(id +
  1)` phantoms with their `finalizeSpawn`, made where the position lies by 4.1's rule
  (in a copy by `send_made`). The draws from the player's stream aside for the tick.
  **Time since rest** (`TIME_SINCE_REST`; n7d ServerPlayer 1913, 2879; finding 42): a
  player's ticks awake while in the game: it does not count while they sleep, and is
  reset when sleep starts and at a death (never, in creative). Kept as the ticks counted
  before plus the region's tick it counts on from (none while asleep), the region's ticks
  as for effects (decision 8 of section 18), carried as the ticks counted at a hand-over
  and a merge, saved in their place (N9d's format raise), reset in `region/sleep.rs`.
- **Patrols** (D9 5.4): at each firing (12 000 + `nextInt(1200)`), by day, `nextInt(5)
  == 0`, a player of the world's table chosen by `nextInt(size)` in the table's order (by
  player id): the region that holds that player acts, the others do nothing; not a
  spectator; not near a village (9.6, question 8); the offsets `(24 + nextInt(24)) ·
  ±1`; every chunk within 10 blocks of the position the region's own land (the game's
  `hasChunksAt(±10)`, 4.1); `can_pillager_patrol_spawn` (the `early_game` timeline: from
  tick 120 000 of the overworld clock; not in mushroom fields); then the members' loop
  (n9 PatrolSpawner 313–428; finding 23): for each of `ceil(eff) + 1`, y set to the
  `MOTION_BLOCKING_NO_LEAVES` heightmap's at the position; `spawnPatrolMember`
  (`isValidEmptySpawnBlock`, `checkPatrollingMonsterSpawnRules`, made, the first the
  leader with `findPatrolTarget`, `finalizeSpawn` with `PATROL`), **a failed leader
  ending the loop** and a failed member not; then x and z each stepped by `nextInt(5) −
  nextInt(5)`.
  Where two regions hold tables a second apart that differ, a firing may make no patrol
  or two (named, rare: it needs a player to join or leave in that second).
- **Cats, the village siege, the wandering trader**: N10 (named). The siege's daily
  `nextInt(10)` on the level's random is not drawn (Clustine keys its streams otherwise).

---------------------------------------------------------------------------------------
## 10. What clients are sent (the edge, A5)

The worker never sees a packet; it reports values and events, and the edge writes them.
New or changed, beside M9 10's table:

| What | Reported as | Written as |
|---|---|---|
| a stack's components | `ItemStack.components` wherever a stack goes | section 3.3 |
| living flags (index 8: using, off hand) | a mob's value; a player's `RegionEvent::PlayerData` | entity data, to all but the player themself |
| effect particles (index 10) and whether all are ambient (index 11) | a mob's value; `PlayerData` | the particle list's serializer (`entity_effect` with a colour, and the four simple particles of 5.2), with index 11 beside it |
| glowing, invisible (shared flags 6, 5) | a mob's flags (invisible built); `PlayerData` | the shared flags byte; **for players the byte whole** (on fire, crouching, sprinting, swimming, invisible, glowing, fall flying, as Clustine has them; decision 6 of section 18), which the edge sends for no player today (`tracking.rs` 461–466) |
| a player's own effects | `PlayerEvent::EffectAdded`, `EffectRemoved` | `UpdateMobEffect`, `RemoveMobEffect`; every effect again at a join (finding 36) |
| a player's own movement attributes | `PlayerEvent::Attributes` | `UpdateAttributes` for their own entity |
| a cooldown | `PlayerEvent::Cooldown` | `Cooldown` |
| a mob's swing | `RegionEvent::ArmSwung` (built for players) | the swing packet |
| zombie's arms, aggressive | mob flags (15) bit 4 (built) | as now |
| arrows | data 8 flags, 9 pierce, 10 in ground, 11 colour; position and rotation **by the type's update interval, 20** (finding 36: `ServerEntity` forces a full position only on that cycle or at an impulse, and the edge already sends by each type's interval, `tracking.rs` 572–646), motion every tick (`trackDeltas`); the add packet's data the owner, through `wire.of` | |
| tridents | 8–10, 11 loyalty, 12 foil | |
| the arrow count | a living value, 12 | |
| an arrow hitting a player of a player | `ClientboundGameEvent` 6 to the shooter (never in creative: the hurt is refused) | |
| the cloud | 8 radius, 9 waiting, 10 particle | |
| fangs | entity event 4 | |
| rockets | 8 stack, 9 attached (empty), 10 shot at an angle; entity event 17 | |
| the guardian's beam | data 17 **an entity's key** (`DataShown::Entity`), which the edge maps to its wire id **by `wire.of`** (`fanout.rs` 927), which gives an id to an entity not yet shown, so the beam does not stick at 0 when its target is shown after the guardian (finding 36: `wire.known`, `fanout.rs` 946, returns none then); projectiles' owners likewise | |
| the pillager | 17 charging | |
| the evoker | 17 the spell | |
| the vex | 16 flags | |
| the slime | 16, 17, 18 size | |
| the enderman | 16 carried block state or none, 17 creepy, 18 stared at | |
| the phantom | 16 size | |
| splash, bottle | level events 2002/2007 with the colour, 1053/1054 | built kinds |
| a tipped arrow's fading | entity event 0 | |
| `TakeItemEntity` | for arrows picked up (built for items) | |
| eating and drinking | the crumbs (`RegionEvent::Particles` of an item particle with the stack) and the sounds at the user (`EntitySound`) | the level particles packet with an `item` particle; `EntitySound` |
| oozing, weaving, infested, wind_charged | level event 3018 (a cobweb), the burst's explosion packet and particles | as built for explosions |

The new `DataShown` variants (`shown.rs` 16–60): `Entity(Option<EntityId>)`,
`OptionalBlockState`, `Particles(Vec<…>)` (so `DataShown` loses `Copy`, or particles are
a list of at most a few colours kept inline: the lead's choice in N9b), `Stack`,
`OptionalUnsignedInt`. The edge also starts sending `EntitySound` (dropped today,
`fanout.rs` 3347), which N9's sounds at an entity use, rather than adding more
`RegionEvent::Sound`s at positions.

---------------------------------------------------------------------------------------

## 11. Comparisons with the official server

On a quiet machine, `#[ignore]` and named `official_server`, with the jar `cargo datagen`
keeps, beside `mobs_oracle.rs`. Exact where nothing draws, by distribution where draws
decide. **Both servers are given the same world seed** (decision 3 of section 18: the
official server's `level-seed` set to Clustine's flat world's), and **neither is driven
by commands** (finding 35; decision 9 of section 18): Clustine has none, so each
comparison reaches its starting state by what a bot can do on both servers (an egg used,
a block placed, a dispenser powered, a stack set in the creative inventory, which may
carry any component of the set, an enchanted crossbow or a loyal trident among them), or,
where only a command could make the state, Clustine's side is the simulation itself, fed
the official server's starting state as recorded from its packets through a test-only
input of the region (`Region::put_entity_for_test`, `cfg(test)` or behind the tests'
feature as the other test-only inputs are), the official server's later packets compared
with what the simulation then does, within the packets' own quantisation.

1. **Components on the wire**: a creative bot sets slots with a tipped arrow, each of the
   tab's potions (drinkable, splash, lingering), a rocket of flight 3, a loaded crossbow,
   an ominous banner, a suspicious stew and an ominous bottle, and puts each in a chest
   and drops one of each; **a second bot** opens the chest and sees the dropped items,
   and both servers send it the same `ContainerSetContent` and `SetEntityData` bytes (a
   creative slot set echoes nothing to its own bot: finding 17); a click on each in the
   chest is answered with no correction by either (the hashes).
2. **A dispensed arrow's flight** (replacing "a summoned arrow"): a bot places a
   dispenser facing up and one facing sideways, fills them with arrows and powers them.
   The arrow's motion is drawn, so the servers' arrows differ; the official server's
   arrow's add packet and its updates give its state after its first tick (position to
   the bit at each full sync, motion as sent), and the simulation, fed that state,
   predicts every later tick: positions and rotations are compared within the move
   packets' 1/4096, and the tick and block it sticks in exactly; the same into water; its
   fall when a bot breaks the block under it (the three draws: the fall's motion within
   the range only).
3. **A refused arrow** (replacing "an `Invulnerable` mob"): a creative bot stands in
   front of the sideways dispenser; its arrows are refused by the player on both servers
   (creative is invulnerable): the motion × −0.1 and a yaw turned by 170 to 190, the
   arrow living on and falling.
4. **An egg's, a splash potion's, a bottle's and a pearl's flight**, thrown by a bot: the
   official server's trace fed to the simulation as in 2, compared tick by tick; the
   potion's level events, and the effects' durations on mobs made by eggs touching the
   potion's box (proximity 1.0) and three blocks off.
5. **The lingering cloud**: thrown by a bot at its feet: its radius every tick and the
   ticks it applies at, to the bot (whose effect packets say when).
6. **Evoker fangs** (replacing "summoned with a fixed warmup"): an evoker and a villager
   made by their eggs in a pen; for each fang, the ticks from its add packet to its
   entity event 4 and to the villager's hurt, which are fixed by the fang's place in the
   spell, compared between the servers; when the spell starts is drawn and is not.
7. **The crossbow's load tick** while held by a bot: the slot's `charged_projectiles` on
   the 26th tick, and the loading sounds on the 6th and 14th.
8. **A skeleton's rhythm** against an iron golem, both made by eggs: arrows every 60
   ticks after the first 20; a pillager's charge of 25 and its shot.
9. **The wire of each projectile**: the add packet, its data and its updates (at the
   type's interval), for an arrow, a spectral arrow, a trident, each thrown kind, a
   rocket, the cloud, the fangs, the llama's spit.
10. **A player's effects**: a bot drinks a potion of swiftness: `UpdateMobEffect`, the
    attributes, the particles and index 11 seen by a second bot, the drinking sound and
    no crumbs; its end; and a bot eating rotten flesh, the crumbs and the sound.
11. **Spawning, by distribution**: the classic flat world of both servers with the same
    seed (slime chunks), one creative bot standing in the open for a whole day (24 000
    ticks) at fixed places; mobs spawned by type and height against the official
    server's, within the counts' noise; and the caps reached (70 monsters about the bot
    at night). Slow; run alone.
12. **The guardian's beam**: a guardian and a squid made by their eggs in a water pen:
    entity event 21, the beam's target id resolved, and the strike's tick.
13. **The bow's draw** (finding 54): a bot draws for 5, 10 and 20 ticks and releases;
    the arrow's speed (its motion's length within the shot's spread) and its crit flag at
    full draw.
14. **Multishot**: a bot sets a crossbow with `enchantments: multishot 1`, loads and
    shoots: three arrows, their angles (0, −10, +10) within the spread, one sound each.
15. **A loyal trident's return**: a bot sets a trident with loyalty 3, throws it at the
    ground; the ticks until it turns back and its return motion, fed to the simulation as
    in 2.
16. **A rocket's lifetime**: rockets of flight 1, 2 and 3 placed by a bot: the ticks from
    the add packet to entity event 17 within `10 · flight + [0, 11]`, by distribution.
17. **A dispenser's shot of each kind** (arrows, eggs, snowballs, potions, the bottle, a
    rocket): its position and the shot's speed by the table of 4.9, and the level events
    1002 and 1004.

---------------------------------------------------------------------------------------

## 12. The division of work

Steps, each one verified commit to `main`, pushed (the four commands of CLAUDE.md in a
clean checkout, the worlds in `/dev/shm`). The lead's first, in this order, each pushed
before anything that builds on it is delegated (a worktree starts from `origin/main`).
**The format rises step by step** (finding 18; decision 4 of section 18): the store's
`FORMAT_VERSION` (13 now, `clustine-format/src/lib.rs` 52) where a step changes the
shape of a record a world already has, and the worker's `STATE_FORMAT` (16 now) in every
step that changes the shape of `RegionState` or of a delta; a new data tag or a new
effect tag needs no store raise. The numbers below are those expected if the steps land
in this order.

| Step | What | Whose |
|---|---|---|
| N9a | **Components and the stack** (section 3): `ItemStack` without `Copy`, `clustine_world::components` and their structure function, `same_kind` at every merge, the defaults generated, the potions table generated from the dump (n7k Potions); **every writer of a stack** (3.2: `put_stack`, the item entity's record, `Interact.held`, `Given`, `LoggedPlace`'s stacks, containers' NBT); the wire's codecs for the set both ways in `clustine-protocol/src/components/`, the `data_component_type` ids generated, the serverbound decoding and the edge's validation, **the clientbound reader** and its readers moved onto it (`Reader::item_stack`, the bots, `items_oracle.rs`: finding 16). Store format 14, `STATE_FORMAT` 17 | the lead |
| N9b | **The projectiles' contract** (4.1): the records and their data tags 14 to 18, `make_projectile` and 4.1's rule for makings across a line, `Local::Shoot(Shot)`, the shared target list with faces, the kinds' ticks dispatched to stubs in the delegates' files; **the player's throws moved out of `region/thrown.rs` into `region/throws.rs`** (S4's), **the block-hit hook's signature fixed** in `region/projectile_hits.rs` (S2's) with the button's arm in `region/redstone.rs` calling it; the `items` module and its files declared with their signatures (`items/enchantments.rs`' reads that S2 calls among them); `make_fangs`, `make_cloud`, `make_firework` declared; `Behaviour::Projectile`'s arm in `region/dispensers/behaviours.rs`; the new `DataShown` variants. `STATE_FORMAT` 18 | the lead |
| N9c | **The use state** (5.5), `PlayerInput::ReleaseUse` and the edge's `RELEASE_USE_ITEM`, the living flags, the cooldowns (in the region's ticks); the trader moved onto it (not the witch). `STATE_FORMAT` 19 | the lead |
| N9d | **The player's effects** (5.4): the store with ends in the region's ticks, its hand-over, merge and save, the ticking, the events and their packets, the time since rest with them (9.7). Store format 15, `STATE_FORMAT` 20 | the lead |
| N9e | **The entity effects** (8.3): `Hurt` (with the hit's stream, `Random::of_hit`), `Alert`, `Effects`, `PearlLanded`, `PatrolTarget`, the holder's side for mobs and players, and the path to a player's stay through the store (`Effected::OnPlayer`). `STATE_FORMAT` 21 | the lead |
| N9f | **The first mob commit** (8.1, 8.2, 7): every target goal real with its table of draws, `alertOthers`, `doHurtTarget` with its extras where the hit lands (8.2), a mob's swing from `MeleeAttackGoal`, `difficulty_at`, the arrow count's part; **`MeleeAttackGoal`, `RangedAttackGoal` and `Raider$HoldGroundAttackGoal` made real** in `mobs/ai/goals.rs`; `RaiderState` split by type out of `illagers.rs` (the witch's, the pillager's, the vindicator's and the evoker's state each in its type's file) with `register_raider_goals` and `register_targets` taking each type's own; **every new module and every new type's arm declared** (`mobs/types.rs` `built()`, 1331; `mobs/types.rs`' module list; `mobs/ai.rs`'), each new type a stub in its part's file; stubs with their signatures for `region/teleports.rs`, `region/infested.rs` (called from `region/drops.rs` and `region/explosions.rs` by the lead), `region/effect_deaths.rs` and `region/wind_bursts.rs`; the streams' tests changed | the lead |
| N9g | **The controlled vehicle** (section 6) | the lead |
| N9h | **Spawning's frame** (9.2, 9.3 but the rules, 9.5) in `region/spawn_caps.rs`: the counts reported, kept by (region, chunk) and published (store, rpc, worker), the caps, the spawning chunks and their order, the refusals at a line, the nearest player with the table indexed by chunk, despawning with it; **the game seed** (every world's, kept in the identity, `Restored` carrying it, the flat world's login hash) and **the zoomed biome**; the chunk's structures field (its writer's call in `services/worldgen/src/stages.rs`, S10's `structures_kept.rs` stubbed); the `spawning` module declared; the merge for targets (8.5). Store format 16 | the lead |

Then, in parallel, each in a worktree of its own. **No file is touched by two delegated
parts** (finding 20): each part's files are listed whole; a function one part calls in
another's file is declared by the lead with its signature before either begins, returning
a neutral value until its owner fills it; a part that finds it needs a change in a file
not its own asks the lead, who makes it on `main`, and the part merges it `--ff-only`.
Every existing test file that the streams' change touches is the lead's in N9f; each part
writes its tests in files of its own (`*_n9_tests.rs` beside its code). Paths are under
`crates/clustine-sim/src/` unless a crate or service is named.

| Part | What | Files (all of them) |
|---|---|---|
| S1 | Effects' rules (5.2) and refusals, particles, the four effects at a death or a hurt and the wind burst, eating and drinking (5.5) | `mobs/body/effects.rs` (after N9d), `mobs/body/effects_n9_tests.rs`, `region/effect_deaths.rs`, `region/wind_bursts.rs`, `items/consumables.rs` |
| S2 | Arrows and tridents (4.2, 4.3), their pick-up, `applyEffectsFromBlocks`, the blocks' projectile hooks | `region/arrows.rs`, `physics/arrows.rs`, `region/projectile_hits.rs`, `region/arrows_n9_tests.rs` |
| S3 | The thrown kinds' hits (4.4), the directed orbs, the cloud (4.5), rockets (4.7), fangs (4.8), the llama's spit | `region/thrown.rs` (after N9b), `physics/thrown.rs`, `region/orbs.rs`, `region/clouds.rs`, `region/fireworks.rs`, `region/fangs.rs`, `region/spit.rs`, `mobs/types/llama.rs`, `region/thrown_n9_tests.rs` |
| S4 | The items' rules in a hand (4.6; the bow's and trident's release for players only), the player's throws, a rocket used on a block, enchantments (4.10) | `items/bow.rs`, `items/crossbow.rs`, `items/trident.rs`, `items/throwables.rs`, `items/rockets.rs`, `items/enchantments.rs`, `region/throws.rs`, `items/items_n9_tests.rs` |
| S5 | Dispensers (4.9) | `region/dispensers/projectiles.rs`, `region/dispensers/projectiles_n9_tests.rs` |
| S6 | The zombie family, the drowned, the creeper, the spiders, the vindicator; the spear if decided in | `mobs/types/zombie_family.rs`, `zombie.rs`, `husk.rs`, `zombie_villager.rs`, `drowned.rs`, `creeper.rs`, `spider.rs`, `cave_spider.rs`, `vindicator.rs`, `mobs/ai/spear.rs`, `mobs/types/melee_n9_tests.rs` |
| S7 | Skeletons, the pillager, the witch, the guardians (the bow's and trident's release for players only); the loot functions with components | `mobs/types/skeletons.rs`, `skeleton.rs`, `stray.rs`, `bogged.rs`, `parched.rs`, `pillager.rs`, `witch.rs`, `guardians.rs`, `guardian.rs`, `elder_guardian.rs`, `mobs/ai/ranged_weapons.rs` (`RangedBowAttackGoal`, `RangedCrossbowAttackGoal`), `mobs/body/loot.rs`, `mobs/types/ranged_n9_tests.rs` |
| S8 | The evoker, the vex, the wololo | `mobs/types/evoker.rs`, `vex.rs`, `mobs/ai/spells.rs`, `mobs/types/spells_n9_tests.rs` |
| S9 | The slime, the enderman, the phantom, the endermite, the silverfish; the frog eating slimes; `randomTeleport`; infested blocks | `mobs/types/cube_mob.rs`, `slime.rs`, `enderman.rs`, `phantom.rs`, `endermite.rs`, `silverfish.rs`, `frog.rs`, `frog_ai.rs`, `region/teleports.rs`, `region/infested.rs`, `region/enderman_blocks.rs`, `mobs/types/n9_mobs_tests.rs` |
| S10 | Spawning's rules (9.4, the sulfur cube's placement and rule among them), the per-chunk spawner on N9h's frame (9.3), the structures written by generation (9.6) | `spawning/rules.rs`, `region/natural_spawning.rs`, `spawning/rules_n9_tests.rs`, `services/worldgen/src/structures_kept.rs` |
| S11 | The custom spawners (9.7) and patrols (8.6) | `region/custom_spawners.rs`, `mobs/ai/patrol.rs`, `region/custom_spawners_n9_tests.rs` |
| S12 | The edge (section 10): the new data, living flags, the players' shared-flags byte, particles and index 11, the beam's id, the hashes, the effects' packets | `services/edge/src/tracking.rs`, `services/edge/src/effects.rs`, `services/edge/src/component_hash.rs` |
| S13 | Tests from the specification (section 14): part A after N9a to N9e and N9h, part B after the S parts | `region/tests/n9_*.rs`, `bin/clustine/tests/n9_*.rs`, `services/worldstore/src/n9_counts_specification.rs` |
| S14 | The comparisons of section 11, written for the lead to run (an agent starts no official server) | `bin/clustine/tests/components_oracle.rs`, `projectiles_oracle.rs`, `effects_oracle.rs`, `spawning_oracle.rs` |

Order: N9a to N9f in turn; S4 and S12 may begin after N9d, S2, S3 and S5 after N9e, S1
and S6 to S9 after N9f, S10 after N9h, S11 after N9h and S9 (its phantoms), S14 after
N9e; N9g at any time after N9f, the lead's. Each brief states the interface, the tests
expected and how to verify (`cargo test -p clustine-sim` or `-p edge`, fmt and clippy in
its worktree); says that the agent does not commit to `main`, push, download, or start
Docker, kind or the official server, and that it removes its worktree's `target` when
done. The lead reads and tests what comes back before merging, and runs the full check
before each push.

**Not delegated**, where ordering mistakes hide: the hand-over of a player's use,
effects, cooldowns and time since rest (N9c, N9d); the holder's side of every entity
effect, the path to a player's stay, and the arrow's fate at a line (the sender's
decision is a call S2 makes into N9e's function, not its own code); where a hit's extras
run and the hit's stream (N9f); the counts' accounting across regions and the refusals
at a line (N9h); the controlled vehicle's write-back within the tick (N9g); the merge
(8.5); `fanout.rs` and the protocol's packets (finding 20: the arms for `owner_of`,
`DataChanged`, `EntitySound`, a mob's `ArmSwung` and the player events are the lead's,
written in the step that makes each event).

---------------------------------------------------------------------------------------

## 13. Records and the roadmap

- **ADR-0028 amended** (as N5 and N7 amended it): section 7 gains the entity effects of
  8.3, players as their addressees by the path to their stay through the store, the
  hit's extras where the hit lands with the hit's stream, and the targets' merge as
  built; section 8 is rewritten as decision 12 has it (counts reported with commits and
  not logged, kept by region and chunk with full reports replacing a region's set,
  others' totals and per-player sums published to each region, nothing published after
  a store restart until every region has reported, the spawnable count computed by
  regions, the lateness). The amendment is reviewed with this plan.
- **ADR-0022 amended**: a chunk carries the starts of the structures with spawn
  overrides and the villages (9.6).
- **`docs/world-format.md`**: formats 14 to 16 as each step raises it, the components'
  store encoding by name, and containers' components in the game's NBT (question 15).
- **The game seed** (decision 3 of section 18): where `docs/architecture.md` and the
  `--seed` help (`bin/clustine/src/main.rs` 80–91) say a world without a seed is flat,
  they say the flat world has a seed too.
- **What to try**, written into the roadmap's first section when N9 is done (`cargo run
  --release -p clustine -- --seed 3 --pin 4 --reshape by-hand`, a new world):
  1. Stay out at night in the plains and in a cave: zombies, skeletons, creepers and
     spiders come within a minute, a witch now and then, an enderman carrying a block;
     at dawn they burn. Walk 130 blocks away and back: they are gone.
  2. A village at night: zombies go for the villagers, the iron golem fights them;
     skeletons shoot the golem; build a snow golem beside a creeper and watch.
  3. Your bow (hold, release), crossbow (hold until it is loaded, use again), trident
     (hold, release; a loyal one comes back), eggs, snowballs, splash potions at a cow
     and at your feet, a lingering potion (the cloud), experience bottles, ender pearls
     (a second's cooldown shown; with pins, throw one across x = 64), a rocket placed on
     the ground. Pick up your arrows. Shoot an arrow at a wooden button.
  4. Drink potions of swiftness, leaping, slow falling, invisibility (a second player
     sees you vanish), night vision; milk takes them away. Poison and harming do nothing
     to you. Eat a golden apple, rotten flesh, a suspicious stew and a chorus fruit;
     hear and see the eating. Splash a potion of oozing or weaving at a zombie and kill it.
  5. Fill a dispenser with arrows, eggs, potions and rockets and power it.
  6. Swamps: slimes and a witch at its hut; a monument: guardians; an outpost:
     pillagers; a desert at night: husks; snowy plains: strays; the sea: fish, squid and
     dolphins; a cave: bats.
  7. With pins: a skeleton west of x = 64 shooting a golem east of it; a witch's splash
     across it; zombies chasing a villager over it. Without pins: two players, one walks
     away until the region splits, then use a zombie spawn egg by the line with a
     villager across it (a creative player cannot lead a zombie: finding 57): the regions
     merge at once (the coordinator's log says why).
  8. Play an hour without sleeping and stand under the night sky: phantoms. Play on into
     the fifth day: a patrol, its leader with the banner.

---------------------------------------------------------------------------------------

## 14. Tests from the specification

Written from M9, D9 and this plan by someone who wrote none of the code (S13), as N7's and
N8's were; each named as a sentence. Part A (the lead's contract):

- a stack with each component of the set survives the store through **every writer**
  (an item entity, a place, an interaction's held stack, a gift, a chest in a chunk file
  in the game's NBT), a hand-over, a hopper and an item entity's merge, and does not
  merge with one that differs;
- the wire's bytes for each component as written down from the game's codecs, both ways;
  the clientbound reader reads a stack with components and names an unknown one; a
  client's components outside the set dropped, malformed ones refused;
- a player's use starts, ticks, is released (`ReleaseUse`) and goes on across a
  hand-over; a use stops when the hand's item changes; a held crossbow loads on its 26th
  use tick into the hand's own stack;
- a player's effects: added, ticking, ended, saved and back at a join with their
  remaining time, moved at a hand-over and a merge, given across a pinned line by a
  splash; **an effect's interval ticks and its end unchanged when the world clock jumps
  forward by a second** at a publication, and when the player arrives in a region whose
  clock is a second ahead; a cooldown and the time since rest likewise;
- each entity effect taken by its holder, for a mob and for a player, refused where the
  target has gone; an effect addressed to a player whose stay is in another region than
  the land they stand in reaches them;
- **a hit's extras where it lands**: a zombie's fire drawn from the hit's stream, the same
  in one region and across a pinned line for the same attacker, `lived` and target, and
  not drawn on a refused hit; a husk's hunger at easy added for one tick;
- the target goals that are `NearestAttackableTargetGoal`s with an interval draw
  `nextInt(5)` on each asked selector tick (the five types of M9 0.4 among them), and the
  enderman's, the phantom's, the vex's and `HurtByTargetGoal` draw nothing (the N9f
  table); they find no creative player, find a villager, a golem, a baby turtle on land
  as the table says; a hurt zombie alerts zombies, husks and drowned within the box, a
  husk only husks, matched by exact class, also across a pinned line;
- a chicken jockey chases its rider's target; a spider jockey's skeleton strafes it; the
  vehicle moves by what its rider wrote the tick before;
- the counts: a region with no publication spawns nothing; the global cap filtered once
  at the start of the phase and not closed by the region's own spawns within it; a
  player's local cap reached from the others' published sums and the region's own spawns
  in one tick; a pack refused across a pinned line; in the store, a lend's "nothing"
  after the borrower's report, a take-over that loads other chunks, a merge and a split
  each leave the counts right, and nothing is published after a store restart until
  every region has reported;
- the game seed: a flat world made has one, kept at reopening, reaches regions in
  `Restored`; a flat world of the identity before is refused.

Part B (the delegated parts): an arrow's tick as D9 1.1, its hit order along a step (a
full pierced set discarding it before any draw, then the crit, then `setLastHurtMob`), its
piercing, its refusal (× −0.1, one draw), the enderman's refusal (and a passenger
enderman's hurt), its block hit moved back per axis, its pick-up rules, the tipped arrow
fading at 600, an arrow pressing a wooden button and a plate and lit by lava; a trident's
bounce either way, its flying on through an enderman, and its loyal return; each thrown
kind's hit and draws (the egg's two, the bottle's two and the orbs' merges, the pearl's
96 and one); the splash's proximity box to box and an effect of exactly 20 ticks dropped;
the cloud's radius and its 5-tick applications, committing only when it applies; fangs'
event 4 and damage ticks; a rocket's lifetime draws, its damage by distance to the entity
it hit and to others, and its pop; a dispenser's position, power and uncertainty for each
kind, drawing nothing from the dispenser's stream; each effect's interval and rule,
resistance after armour, the refusals (the wither skeleton's, the nautilus's, the two
immunities), the undead inversion; oozing, weaving, infested and wind_charged with their
draws; every food's and drink's consume effects with their probability draws, crumbs and
sounds; the bow and the trident running nothing at a mob's release (a skeleton shoots
once); the witch's drinking off the use state; each monster's rhythm at easy (M9 0.11),
its draws by a counting `Draws`; the slime's size and split, the enderman's take and
leave (also across a pinned line, refused where the block changed), its daylight
teleport; the silverfish's merge and waking; the vex's life; the evoker's three spells;
each spawn rule's draws in order (D9 5.7), the pack size replaced at the type's pick, the
cluster sizes, a mob refused at its position taking no key, the slime chunk's seed, the
structure overrides (witches only at the hut, nothing in an ancient city), the village
stand-in's 32 blocks, a type Clustine lacks ending the position only after its own
checks; phantoms by time since rest (not counting asleep), the spawners' firings caught
up after the clock jumps, patrols by day 5 with the member loop; despawning by the
players' table.

---------------------------------------------------------------------------------------

## 15. Cost: what is measured, and when it is acted on

Measured as N7f measured herds, in memory and on the real disk with its cache, each
number said with where it is from:

- **A night's monsters** about one player (70 monsters, moving): the log's growth a
  minute (each moving mob commits about 83 bytes a tick, N7f: about 7 MiB a minute for
  70), and the copies' updates across a line. Acted on if it is more per mob than N7f's
  pen; the plan's cheaper rule (A28 section 3) is the remedy named there.
- **Spawning's work a tick** (finding 33): the caps do not refuse most of it. By day, in
  an empty sea, or with few bats, every spawning chunk (about 200 a player) draws for
  each open category every tick, and each attempt asks for the nearest player. Measured
  in that case, with 100 players (the table's players, the region's own a few), and the
  tick's length with four players' nights in one region; acted on if a region's tick
  passes 10 ms of spawning, by the index of players by chunk (9.3) first.
- **The counts**: what a commit carries (one entry per chunk whose counts changed) and
  what the store publishes a second to each region (seven numbers per player and the
  totals, over the others' chunks): small; measured with 100 players' and ten regions'
  worth of fake reports.
- **Projectiles at rest**: an arrow in the ground commits nothing (derived counters);
  checked by a test, not measured.
- **A crossbow volley at a line** and a horde's merge: the merge's pause measured as
  N7f's.

---------------------------------------------------------------------------------------

## 16. Named differences

Collected from above; each is said in the code where it is made.

- Spawning's counts from other regions about a second late; the global cap may be
  overshot by a second of other regions' spawning (9.2).
- The order of spawning chunks is the game's in distribution, not its draws (9.3).
- A pack does not step into another region's land (decision 13).
- A type Clustine does not build ends a spawning position (the sulfur cube: about a third
  of sulfur caves' first picks).
- Inhabited time is not kept; generation's mobs read the first day's difficulty (7).
- An arrow that hits an entity across a line is gone, where it would have turned back on
  a refusal; a piercing one ends there (decision 4).
- A hit's extras (the zombie's fire, the husk's hunger and the rest) are drawn from the
  hit's stream where the hit lands, not from the attacker's own random in its tick
  (decision 18, 8.2); a mob's weapon does not wear from a hit across a line.
- An alert, an effect, a teleport or a making that reaches past the copies is lost (8.1,
  8.4, 4.1); a patrol is not made within 10 blocks of a line (9.7).
- An enderman's take across a line refused at the holder leaves it holding a duplicate;
  its put-down refused loses the block (8.4).
- An effect addressed to a player whose stay is between regions is missed (8.3).
- A spawning attempt where the light is not yet known fails (9.3).
- Spawning pauses after a store restart until every region has reported (9.2); a chunk
  that just changed hands may be counted twice for about a second.
- A custom spawner's firing in a hand-over's tick may be missed or doubled (9.7).
- A pearl whose thrower's stay is between regions when it lands, or who left the game,
  teleports nobody (4.4).
- A patrol may be missed or doubled where the players' table changed in the second of
  its firing (9.7); villages stand in for points of interest in the patrol's test.
- The crossbow's loading-sound flags are per region, not per server (4.6).
- A returning trident follows a face a tick old or a table entry a second old (4.3).
- No rocket from the hand (no elytra); no riptide; no endermite from anything but a
  pearl; no sulfur cube; bad omen does nothing (no raids).
- Components outside the set are dropped (section 3; 1.4 names what a player meets).
- The village siege, cats in villages and the wandering trader's spawning wait for N10.

---------------------------------------------------------------------------------------

## 17. Open questions, each with my recommendation

As the draft put them. Section 18 gives the review's answer to each and what the plan now
says, and the sections above are corrected to it; where an answer departs from the
recommendation here, a line below says so.

1. **Components outside the set.** A client's components are length-prefixed, so the
   edge could keep any of them as opaque bytes and send them back. But the vanilla server
   decodes and validates every component, and bytes passed on unread could carry what
   crashes another player's client. *Recommended*: the typed set only, the rest dropped
   and named (as today); a kind is added when something needs it, without a format raise
   (3.2).
2. **The component hashes in menu clicks.** *Recommended*: computed for the set, checked
   against the official server (comparison 1); without them every click with a potion in
   a chest is corrected, which nobody sees but costs a packet a click.
3. **The sulfur cube.** *Recommended*: out, named, with its share of sulfur caves'
   positions; its class is 2,377 lines and its own registry, not small. Alternatively the
   spawner could skip a type it lacks rather than end the position, which keeps the other
   monsters' share in those caves but departs from the game's draws; not recommended.
4. **The endermite**, and the dumps still owed: `Endermite` (not dumped), `Mob.finalizeSpawn`'s
   random (D9 5.8), `LivingEntity.isInvertedHealAndHarm`, `SpearUseGoal` (dumped, unread),
   the vex's data index. *Recommended*: the lead dumps them in N9a; the endermite in N9
   (a pearl in twenty makes one, and a creative player throwing pearls meets it within
   minutes), with S9. *Answered*: most were dumped already (finding 58); the lead dumped
   the rest after the review; what is still owed is listed in section 18.
5. **Every category spawning, not only monsters** (section 0, added). *Recommended*: in
   N9; an empty sea is a seam a player meets at once, and the code is the same.
6. **Players addressed by entity effects** (8.3) rather than a new path through the
   store. *Recommended*: so; it reuses A25's and A28's path, and the rare miss (a player
   who moved chunk in that tick) is the game's own miss of a target that moved.
   *Answered otherwise*: a player's region and the land they stand in can differ, so
   effects to players go to their stay through the store (8.3, finding 27).
7. **The enderman's blocks across a line** as `SetIf`s decided on the copy.
   *Recommended*: so, as the snow golem's trail.
8. **The patrol's village test** without points of interest. (a) always false, named;
   (b) the village structures' boxes kept in the chunk (9.6) stand in. *Recommended*:
   (b): a patrol spawning in a village whose player stands in it would shoot the villagers,
   which a player notices; the boxes are cheap and go when N10 brings points of interest.
9. **The spear goal**: built if reading it shows a day's work or less, else named and its
   holders fall through to their melee goal. *Recommended*: so. *Still open*: the
   review asks the mob's and the player's spear to be decided together (section 18).
10. **One format raise for all of N9**, in N9a, refusing worlds made before (as every
    raise before); a reader of format 13 is not written. *Recommended*: so; the owner
    makes a new world once. *Answered otherwise*: a raise in each step that changes a
    shape (decision 4 of section 18; section 12).
11. **The custom spawners' schedules as functions of the world clock** (9.7) rather than
    a world countdown kept somewhere. *Recommended*: so; nothing of the game's countdown
    is saved either. *Answered*: of game time, with catch-up (finding 24).
12. **The counts not logged** (9.2): rebuilt from regions' reports after a store restart,
    so spawning pauses for a second there. *Recommended*: so; logging them would grow the
    log for what is derived. *Answered*: so, with finding 4's keying and full reports.
13. **Effect particles for mobs too** (5.2), which the edge has never sent.
    *Recommended*: now; a poisoned golem without swirls is a seam once effects exist.
14. **The use state for every item that has a use** (5.5: the shield, the spyglass, food
    shown being eaten), completing only the consumables named. *Recommended*: so; the
    flags cost nothing more once the state exists, and others see a raised shield.
15. **A record of its own?** N9 adds no new kind of state between regions: its entity
    effects are A28's, its counts are A28 section 8's, done. *Recommended*: amend
    ADR-0028 and ADR-0022 (section 13), reviewed with this plan, rather than a new record.

---------------------------------------------------------------------------------------

## 18. The review (2026-10-26), and what the plan is now

An independent review found 22 defects, 18 risks and 18 nits, and answered the open
questions (`n9-review.md` in the session's notes). Each finding was checked against the
code (at `5a5ea15`, where its citations were checked) and the dumps before it was taken.
The sections above were corrected in place wherever a finding changed a number, a rule,
an order of draws, a file's owner or a step; where this section and they still differ,
this one holds. After the review the lead dumped 23 more classes into `n9-javap/`
(`Endermite`, `PatrollingMonster` and its `LongDistancePatrolGoal`, the four effects of
finding 14 with `OozingMobEffect$NearbySlimes`, `KineticWeapon` and its `Condition`,
`Consumable` and its inner classes, `Consumables`, `ConsumeEffect` and its five kinds,
`FoodProperties` and its builder), which this revision reads where the plan relies on
them; `Silverfish` and its two goals were there already.

**Refused**: none outright. Three were taken in another form than proposed, each said
below: finding 30 (the store publishes the other regions' counts, so the region adds its
own as they are now, rather than correcting its own past counts); finding 5 (the lead's
option (b), made uniform); finding 2's aside that "`afterSpawn` raises only the local
counts" is not what the dump shows (n7 NaturalSpawner$SpawnState `afterSpawn` raises the
category's total too), but the total is read only by the filter made before, so the plan
is as the finding asks.

### 18.1 The lead's decisions on what the review left open

1. **Hits that land after the attacker's tick** (finding 5): option (b), made uniform.
   The extras of a hit that apply only if it took (the zombie's fire draw, the husk's
   hunger, wear, `doPostAttackEffects`, the attack sound) run where and when the hit
   lands, inside one region and across a line alike, drawing from **the hit's stream**,
   `Random::of_hit(world, attacker, attacker's lived, target)`: a new stream in
   `region/random_ticks.rs` beside `of_entity_aside` (99–111), with a mark of its own,
   keyed by the target so that a piercing arrow's hits draw apart, and without the
   region's tick, which `of_entity_aside` mixes in and which differs between the
   attacker's region and the lander's. `Local::Hurt` and `EntityUse::Hurt` carry the
   attacker's key and `lived` at the hit. This **replaces decision 18** ("drawn at the
   attacker as if the hit took"). **The change to the game's order of draws**: the game
   draws these from the attacker's own random within its tick; Clustine draws them from
   the hit's stream when it lands, so they leave the attacker's own stream. Sections 0
   (decision 18), 8.2, 8.3 and 16 say so. Wear across a line is named lost (a mob's
   weapon, which nobody sees break at easy), since the lander cannot write the
   attacker's record.
2. **The four creative-tab effects** (finding 14): oozing, weaving and wind_charged are
   built from their dumps; infested is built with the silverfish, which comes into N9
   for it (`Silverfish` and its two goals, S9), with infested blocks' `spawnAfterBreak`
   (an explosion's). Their particles (`item_slime`, `item_cobweb`, `small_gust`,
   `infested`) and draws as dumped (5.2's table). The missing refusals are added
   (`#immune_to_infested`, `#immune_to_oozing`, the wither skeleton's wither, the
   nautilus's poison) and the tag's name corrected to `#inverted_healing_and_harm`.
   Sections 0 (decisions 6 and 11), 1, 5.2, 8.4, 12 and 16 changed.
3. **The seed and the biome** (findings 22 and 34): every world has a game seed, the
   flat world included. **Where it is kept**: in the world's identity, as a seeded
   world's is now (`terrain;features …;seed N`, `services/worldstore/src/lib.rs`
   838–857): the flat world's becomes `flat;classic;seed N`, N the seed given at the
   making or, without one, a fixed default the server picks then (a constant of
   `bin/clustine`, the lead's to choose) and keeps with the world; a flat world of the
   identity before is refused and made again, as the format's other raises refuse.
   **How it reaches regions**: `Restored` (`clustine-rpc/src/messages.rs` 836–846) gains
   `game_seed: i64` beside the features' seed (`seed: Option<i64>`, renamed
   `features_seed`), from the identity by a `game_seed(identity)` beside `world_seed`,
   into `RegionConfig`; the login's hashed seed of a flat world becomes its seed's
   (`bin/clustine/src/lib.rs` 133–144). Slime chunks (9.4) and the zoomed biome lookup
   (`BiomeManager` with the hashed seed, as the game's `ServerLevel.getBiome`, from
   `clustine-terrain/src/biome/zoom.rs`; 9.3) read it, and the comparisons give the
   official server the same `level-seed`. N9h's. Sections 0, 9.3, 9.4, 11, 12 and 13
   changed.
4. **The format** (open question 10, finding 18): raised step by step as each step
   changes a shape, the store's and `STATE_FORMAT` alike (section 12's table gives the
   expected numbers: store 14 in N9a, 15 in N9d, 16 in N9h; `STATE_FORMAT` 17 to 21 in N9a
   to N9e). Sections 0 (header), 3.2, 4.1, 5.4, 8.3, 9.6 and 12 changed.
5. **Foods** (finding 38): every food's and drink's `consume_effects` from the data are
   applied: `apply_effects` with its probability draw, `remove_effects`,
   `clear_all_effects`, and also the two other kinds the data uses, `teleport_randomly`
   (the chorus fruit, through the enderman's `randomTeleport`) and `play_sound` (the
   ominous bottle), since a creative player eats a chorus fruit within minutes and both
   are dumped; with the stack's listeners (`potion_contents`, `suspicious_stew_effects`,
   `ominous_bottle_amplifier`, the latter two added to the set), the eating and drinking
   sounds and crumbs built (`Consumable.emitParticlesAndSounds`). Sections 1.3, 3.1, 4.6,
   5.5, 10 and 12 (S1's `items/consumables.rs`) changed.
6. **The players' shared-flags byte** (finding 36): the edge sends it whole for players
   (on fire, crouching, sprinting, swimming, invisible, glowing, fall flying, as
   Clustine has them), with index 11 beside index 10. Sections 5.2, 5.4 and 10.
7. **The counts** (finding 4): keyed by (region, chunk), a full report replacing a
   region's set, a region's set dropped when it loses its owner or is absorbed, and no
   counts published after a store restart until every owned region has reported.
   Question 12 then as the review answers. Sections 0 (decision 12), 9.2, 13, 14 and 16.
8. **Ends kept in the region's tick** (finding 1): the ends of effects, cooldowns and the
   time since rest are kept in the region's ticks and carried as the ticks remaining
   across hand-overs, the place and merges, as `attack_reset` is (`state.rs` 99–105,
   `region.rs` 1718). Sections 4.4, 5.1, 5.4, 9.7, 12 and 14.
9. **The comparisons without commands** (finding 35): each says how both servers reach
   the same starting state: by what a bot can do on both (eggs, blocks, a powered
   dispenser, any stack of the set set in the creative inventory), or, where only a
   command could make it, by the simulation fed the official server's recorded state
   through a test-only input and compared with the official server's later packets.
   Comparison 2 became a dispensed arrow, 3 an arrow refused by a creative bot, 6 fangs
   timed from their own add packets; 8 and 12 use eggs; none was found that cannot be
   driven so. Section 11.
10. **Everything else as the review recommends**, below, finding by finding.

### 18.2 Each finding

**Defects.**

1. Ends in world game time: **taken** (decision 8).
2. The global cap filtered once a tick before the shuffle: **taken** (9.1, 9.2; checked:
   n7 ServerChunkCache offsets 125 and 180, `spawnForChunk` asks only
   `canSpawnForCategoryLocal`).
3. The pack size replaced at the type's pick, and the cluster sizes: **taken** (0, 9.3;
   checked against n7 NaturalSpawner 297–309 and each type's `getMaxSpawnClusterSize`).
4. Counts by chunk alone: **taken** (decision 7).
5. Where a hit's extras run: **taken as option (b)** (decision 1).
6. The trident at an enderman: **taken** (0 decision 4, 4.3; checked: n9 ThrownTrident
   376–399, n9 Enderman 973–976).
7. The dispenser's power and uncertainty swapped, the rocket's angle, the draws: **taken**
   (4.7, 4.9; checked in n9 ProjectileItem$DispenseConfig$Builder 11–27,
   ThrowablePotionItem, ExperienceBottleItem, FireworkRocketItem 140–165,
   ProjectileDispenseBehavior).
8. The arrow's hit order, the per-axis move-back, the hit sound's reset, `setLastHurtMob`:
   **taken** (4.2; checked at n9 AbstractArrow 999–1064).
9. The rocket's damage by distance: **taken** (4.7; n9 FireworkRocketEntity 573–722).
10. `endsWithin(20)` is `<= 20`: **taken** (4.4; n7 MobEffectInstance 373–385).
11. The witch off the use state: **taken** (0 decision 3, 5.5, 12; n7 Witch).
12. The bow's and trident's release only for players: **taken** (4.6; S4's and S7's
    briefs, 12).
13. Resistance after armour: **taken** (5.2).
14. The four effects and the refusals: **taken** (decision 2).
15. Every writer of a stack, and containers' NBT: **taken** (3.2, 12 N9a, 14; checked:
    `clustine-format/src/entities.rs` 189, 383; `effects.rs` 165–171, 290–296; `log.rs`
    255–260, 1085–1090; `region/containers.rs` 296–338).
16. The clientbound reader: **taken** (3.3, 12 N9a; `clustine-protocol/src/item.rs` 39–51).
17. A creative slot set echoes nothing: **taken** (comparison 1 uses a second bot).
18. One format raise: **taken** (decision 4).
19. `Using` without the stack: **taken** (5.5).
20. File ownership: **taken** (section 12 rewritten: N9f makes the three shared goals
    real, splits `RaiderState` and declares every module and type arm; N9b fixes the
    block-hit hook and moves the player's throws to `region/throws.rs`; the lead writes
    the protocol packets and `fanout.rs`'s arms; every part's files listed whole, none in
    two parts; the frog's files are S9's; `mobs/ai/goals.rs` is one file and the lead's).
21. Arrows in blocks: **taken** (4.2, S2; `region/redstone.rs` 415–417's arm calls S2).
22. The game seed: **taken** (decision 3).

**Risks.**

23. Makings in another region's land, and the directed orb and the patrol's loop: **taken**
    (4.1's one rule; 4.4's orbs; 9.7's member loop as n9 PatrolSpawner 313–428 has it,
    the heightmap `MOTION_BLOCKING_NO_LEAVES`).
24. The spawners' schedules on game time with catch-up: **taken** (9.7).
25. The enderman: **taken** (4.2, 8.3's `Effects` carrying the source's kind, 8.4; the
    duplicate and the lost block named, 16).
26. Patrols across lines: **taken** (`LongDistancePatrolGoal` dumped and read; a new
    entity effect `PatrolTarget`, 8.3, 8.6).
27. Effects addressed to players: **taken**, with the path the review asked for and left
    unshaped: the store delivers an `Effected::OnPlayer` to the region that holds the
    player's stay (8.3). The lead's to confirm (18.4).
28. Alerts: **taken** (8.1: exact class; the loss past the copies named).
29. Not every target goal draws: **taken** (8.1's table in N9f; section 14 no longer pins
    a blanket draw; checked: n9 Enderman$EndermanLookForPlayerGoal).
30. The local cap's mixed times: **taken in another form** (9.2): the store publishes to
    each region the other regions' sums against the positions in the same message, and
    the region adds its own counts as they are now, so no time is mixed and nothing of
    its own is late; a chunk just changed hands may count twice for a second (named).
31. A type Clustine lacks ends the position only after its own checks: **taken** (9.3;
    the sulfur cube's placement, size and rule in S10).
32. Keys for mobs refused: **taken** (9.3: the key and the mob's own draws only after
    `isValidPositionForMob`, which changes no draw of the chunk's stream).
33. Cost below the caps: **taken** (9.3's index by chunk, 15's measurement).
34. The biome: **taken** (decision 3; 9.3).
35. Comparisons needing commands: **taken** (decision 9).
36. The wire: **taken** (10: the interval of 20, `wire.of`, index 11, the effects at a
    join, the players' flags byte; decision 6).
37. Two `ItemStack`s: **taken** (3.1: the world's types in `clustine-world`, the wire's in
    `clustine-protocol/src/components/` with generated ids, the edge converting and
    validating; checked: `clustine-protocol` depends on no Clustine crate).
38. Foods: **taken** (decision 5).
39. The controlled vehicle's write-back: **taken** (6).
40. The merge's "to" region: **taken** (8.5; `arrivals.rs` 117–130).

**Nits.**

41. Difficulty's 1 032 000 ticks and the moon all day: **taken** (7).
42. Time since rest's resets and not counting asleep: **taken** (9.7).
43. Unknown light refuses: **taken** (9.3; the light citations now `region/light.rs`
    365–403).
44. The village stand-in: **taken** (9.6).
45. The swing is `MeleeAttackGoal`'s: **taken** (8.2).
46. Skeletons flee wolves; the witch's targets: **taken** (1.2, 8.4).
47. The husk's hunger of 0 ticks: **taken** (5.3, 14).
48. The wololo needs mob griefing: **taken** (8.4).
49. The crouching box: **taken** (4.1).
50. The egg's variant default: **taken** (3.1, with `chicken/variant` in the set).
51. A cooldown refuses the use: **taken** (4.4).
52. The rocket's block hit runs `entityInside` first: **taken** (4.7).
53. The cloud's radius every tick: **taken**, derived (4.5).
54. Missing comparisons: **taken** (11, comparisons 13 to 17).
55. Dead reasons: **taken** (5.5, 8.3, 4.4).
56. Loot functions with components: **taken** (8.4, S7's `mobs/body/loot.rs`).
57. What to try, item 7: **taken** (13).
58. Dumps owed: **taken** (2's last paragraph, 17's question 4, 18.3).

### 18.3 The open questions of section 17, as answered

1. The typed set only: **agreed**, with 1.4 naming what a player meets within minutes
   (enchanted books, goat horns, named and dyed items, the tab's paintings, the first
   click's correction) and registry values kept by name (3.1).
2. The hashes: **agreed**, for the whole set, from the structure function shared with
   the containers' NBT (3.1, 3.3).
3. The sulfur cube out: **agreed**, keeping its placement, size and count draw (9.3).
4. The endermite in N9 with S9: **agreed**; the owed list corrected (finding 58) and
   dumped by the lead.
5. Every category: **agreed**, with the cluster sizes, every rule that draws, and the
   cost measured below the caps (0, 9.3, 15).
6. Players addressed by entity effects: **agreed in principle**, by a path to the
   player's stay (8.3); the lead's to confirm.
7. The enderman's `SetIf`s: **agreed**, the duplicate and the lost block named.
8. The village test: **(b)**, as finding 44 specifies (9.6).
9. The spear: **open** (18.4).
10. One format raise: **no**, per step (decision 4).
11. Schedules: **agreed**, on game time with catch-up (9.7).
12. Counts not logged: **agreed**, with decision 7.
13. Particles for mobs: **now**, with index 11 and the four special particles.
14. The use state for every item: **agreed**, foods completed (decision 5), the witch off
    it.
15. Amend ADR-0028 and ADR-0022, no new record: **agreed**, and `docs/world-format.md`
    for the components in containers (13).

### 18.4 What the lead must still dump, and what is still open

**Still owed as dumps** (the lead's, in N9a, before the parts that need them):

- `AbstractWindCharge` (its `EXPLOSION_DAMAGE_CALCULATOR`) and
  `SimpleExplosionDamageCalculator`, for wind_charged's burst (S1);
- the blocks' `onExplosionHit` for `ExplosionInteraction.TRIGGER` that are not in the
  dumps (`ButtonBlock`, `LeverBlock`, `BellBlock`, `AbstractCandleBlock`; `DoorBlock`,
  `TrapDoorBlock` and `FenceGateBlock` are in n7), found by a search of the jar for
  overrides of `onExplosionHit` (S1);
- `InfestedBlock` (`spawnAfterBreak`, the host states) for the silverfish (S9);
- `OminousBottleAmplifier` (its `onConsume`) (S1); `SuspiciousStewEffects` is in n7;
- `PiercingWeapon` and the player's stab, if the spear is decided in (below).

**Open, for the lead to decide**, each with a recommendation:

1. **The spear** (question 9). `KineticWeapon` (545 lines), its `Condition` (157) and
   `SpearUseGoal` (390, with `SpearUseState`) are dumped; the player's stab needs
   `PiercingWeapon` and the edge's handling of the stab, which it drops today. The
   spears are in the creative tab, so a player meets a spear that does nothing within
   minutes, today already. *Recommended*: in N9, the player's and the mob's together, as
   a part of its own after N9c (`items/spear.rs` for `KineticWeapon` and the stab, S6's
   `mobs/ai/spear.rs` calling it, the player's input the lead's), once `PiercingWeapon`
   is dumped and read; named and left out only if reading shows more than two days'
   work, the zombie then falling through to `ZombieAttackGoal`.
2. **The path to a player's stay** (finding 27; 8.3). *Recommended*: as written, the store
   delivering `Effected::OnPlayer` to the stay's holder, a stay between regions missing
   it (named); the alternative, the land's holder passing it on, needs a second hop and a
   region that knows other regions' players, which none does. ADR-0028 records it.
3. **Wear across a line** (decision 1). *Recommended*: named lost, as written (8.2),
   rather than an effect back to the attacker's region for a sword's durability.
4. **The flat world's default seed** (decision 3). *Recommended*: a fixed constant named
   in `bin/clustine`, said in `--seed`'s help, so that every flat world has the same
   slime chunks unless given a seed, as one flat world made by the game for a given
   `level-seed` has.
5. **The first number of `Random::of_hit`'s keying** (decision 1). *Recommended*: the
   attacker's `lived` at the hit, which the attacker's region knows and the effect
   carries; an attacker hits a target at most once in a tick, and a piercing arrow's
   targets are told apart by the target's key.

### 18.5 The lead's rulings on what the revision left (2026-10-26)

1. **The spear: in N9**, the player's and the mob's together, as a part of its own after
   N9c, as recommended; `PiercingWeapon` dumped with the rest below. Named and left out
   only if reading it shows more than two days' work.
2. **The path to a player's stay**: as written, the store delivering `Effected::OnPlayer`
   to the stay's holder; a stay between regions at that moment misses it, named.
3. **Wear across a line**: named lost.
4. **The flat world's default seed**: a fixed constant in `bin/clustine`, said in
   `--seed`'s help.
5. **`Random::of_hit`**: keyed by the attacker's `lived` at the hit and the target's key.

The dumps 18.4 lists as owed are made in `/home/claude/reference/n9-javap/` with these
rulings (the jar's `onExplosionHit` overrides found by a search of its classes).
The search found `onExplosionHit` overridden by `AbstractCandleBlock`, `BeehiveBlock`,
`BellBlock`, `ButtonBlock`, `CreakingHeartBlock`, `DoorBlock`, `FenceGateBlock`,
`LeverBlock` and `TrapDoorBlock`; the four the revision named are now in n9-javap with
`AbstractWindCharge`, `SimpleExplosionDamageCalculator`, `InfestedBlock`,
`OminousBottleAmplifier`, `PiercingWeapon` and `CreakingHeartBlock`, and S1 reads the
beehive's from n7.
