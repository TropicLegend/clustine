# N9 porting brief: monsters that fight, projectiles and natural spawning (26.3, protocol 777)

Read from `server-26.3.jar` with `javap -p -c -constants` (names are Mojang's), one file per
class, in `/home/claude/reference/` (`n5-javap/` to `n8-javap/`), from the datagen's output in
`target/datagen/26.3/generated/` (biomes, structures, dimension types, tags) and from
Clustine's code. A file named without its directory is in `n7-javap/`; the creeper, spider,
skeleton, drowned and projectile classes are in `n7k-javap/` and say so where it matters
(`n7k`), the newer `LivingEntity`, `Player` and `ServerLevel` in `n7d-javap/` (`n7d`). Line
ranges are of the `javap` output of the class named. Every rule is the bytecode's or the
data's unless marked **Inferred** (reasoning, or a class that is not dumped, from memory of
the game's source: to be confirmed by the dumps section 14 asks for), **Unverified
(Pumpkin/SteelMC)** (read in the Rust servers under `/home/claude/reference/`, not
authoritative) or **Client**. Cited as K: `n7k-brief.md` (K 11.2 is its section 11.2), B:
`n7-brief.md`, P: `n7k-plan.md`, NP: `n-plan.md`, A25 and A28: ADR-0025 and ADR-0028.

The manner is K's: what the step should know first (section 0), then subject by subject
(1 to 8), the draws (9), what clients are sent (10), the line between regions (11), what a
player meets within minutes (12), what Clustine has to reuse (13), what was not dumped (14)
and the open questions (15). B's conventions hold: `double` is `f64`, `float` is `f32`, a
`float` constant in `double` arithmetic is widened after the float arithmetic it takes part
in, no fused multiply-add. "Own" is the mob's (or projectile's) own random; "level" is the
level's random (`ServerLevel.random`). A "selector tick" is a full goal-selector tick, every
other game tick (B 2.1); `reducedTickDelay(n)` is `ceil(n / 2)`.

Where it sits: NP section 4 names N9 "Monsters, projectiles (arrows, snowballs, eggs,
tridents), burning in daylight, natural spawning with both caps, difficulty". Burning, the
snowball, the monsters' bodies and the difficulty (easy, `Difficulty::WORLD`) were built in
N7k (P sections 10 and 11; the roadmap's "N7k"); light in the simulation is N8 (its first
part, N8a, on `main`). What N9 has left: targets and attacks, the other projectiles, mobs
steered by their riders (P 11 item 1), the mob effects the attacks give, and natural
spawning with A28 section 8's counts.

---------------------------------------------------------------------------------------

## 0. What the step should know before planning (findings)

**Who fights whom**

1. **No monster ever targets, keeps or hurts a creative player, and every player is
   creative.** Three locks, each enough: `Mob.setTarget` and `getTarget` pass through
   `asValidTarget`, null for a `Player` that `isCreative()` or `isSpectator()` and for one
   `!canAttack` (Mob.txt 334–396; checked); `LivingEntity.canAttack(t)` is
   `t.canBeSeenAsEnemy()` (n7d LivingEntity.txt 2162–2176), false for a player whose
   abilities are invulnerable (n7d Player.txt 1603–1615), and `TargetingConditions.test`
   asks it of every combat target (TargetingConditions.txt 111–205); `MeleeAttackGoal`
   stops on `NO_CREATIVE_OR_SPECTATOR` (MeleeAttackGoal.txt 105–203). A creative player's
   hit is recorded (`setLastHurtByMob`, `setLastHurtByPlayer`, n7d LivingEntity.txt
   3340–3414) but `HurtByTargetGoal.canUse` fails its `canAttack`, so the mob does not turn
   on them and **alerts nobody** (HurtByTargetGoal.txt 89–133). Clustine's sensing already
   holds these locks (`mobs/ai/sensing.rs` 131–133, 293, 433–444). N9 needs no path from a
   mob's hit to a player.
2. **So in Clustine's world every fight is mob against mob**, and it is ordinary play in
   and around a village: zombies, husks, drowned, vindicators, pillagers and evokers go for
   villagers and wandering traders (the illagers not for a baby villager:
   `AbstractIllager.canAttack`, AbstractIllager.txt 22–36), and with the skeletons and (in
   the dark) spiders for iron golems; zombies, drowned and skeletons for baby turtles on
   land; drowned for axolotls; guardians for squid, glow squid and axolotls
   (`GuardianAttackSelector`, 14–35); a witch, a creeper or any monster for whatever hurt
   it, which includes a **snow golem's snowball** (a 0-damage hurt still sets
   `lastHurtByMob`: K 11.10), so a creeper walks up to a snow golem and explodes. Golems
   fight back and seek monsters (K 1.2–1.3), now that monsters exist that fight.
3. **What a creative player meets of it directly** is small and exact: an arrow or a trident
   aimed at a golem that hits them bounces off (the hurt fails, `REVERSE` deflection, the
   motion × 0.2: **Inferred**, `AbstractArrow` not dumped); a creeper's explosion pushes a
   walking creative player but not a flying one and hurts neither (n6 ServerExplosion.txt
   `hurtEntities` from 490); a witch's splash at a golem beside them gives them its effect
   (no game-mode test in `addEffect`, n7d LivingEntity.txt 2311–2371); evoker fangs and a
   vex hurt nothing of theirs. And **what they shoot and throw themselves** (section 4),
   which is most of what N9 gives a player.

**Targets and melee**

4. **Five types' streams already miss draws.** The zombie family, the drowned, the illagers,
   the witch and the guardians hold their target slots with `tamable::Idle` (flags 0, never
   draws: `zombie_family.rs` 236–256, `drowned.rs` 377–392, `illagers.rs` 146–157,
   `witch.rs` 163–180, `guardians.rs` 171–195), where the game's
   `NearestAttackableTargetGoal` draws `nextInt(reducedTickDelay(10)) = nextInt(5)` every
   selector tick it is asked, before any search (NearestAttackableTargetGoal.txt 77–99), and
   the witch's `NearestHealableRaiderTargetGoal` a `nextBoolean()` before it asks for a raid
   (its file 38–66). Only the skeletons, the creeper and the spider (`TargetOfN9`,
   `monster_ai.rs` 62–105) draw as the game. Replacing the placeholders changes those
   streams, and the tests that pinned them.
5. **Finding a target always needs line of sight; `mustSee` only governs keeping it**
   (`forCombat()` keeps `checkLineOfSight`, TargetingConditions.txt 1–50; `TargetGoal.
   canContinueToUse` 85–133: 30 selector ticks unseen, 150 for HurtBy, 300 for the evoker).
   `mustReach` does nothing for `NearestAttackableTargetGoal`, which never calls
   `TargetGoal.canAttack` (77–173): the Johnny vindicator's `mustReach` builds no path.
6. **`alertOthers` is by class, through `instanceof`** (HurtByTargetGoal.txt 134–243): a hurt
   zombie sets its attacker as the target of every `Zombie` (husks, zombie villagers, drowned
   included) in `inflate(follow, 10, follow)` that has none, a husk only of husks; it skips
   allies and the exact ignored classes (`ZombifiedPiglin`). Zombies, drowned, vindicators,
   evokers, vexes and the ravager alert; the witch and the pillager do not. Clustine's
   `HurtByTargetGoal` has no alerting (`goals.rs` 1400–1468).
7. **Difficulty decides what a zombie does to a villager**: `Zombie.killedEntity` (859–900)
   converts the villager to a zombie villager only on normal (one own `nextBoolean()`) and
   hard (always); **on easy nothing is drawn and the villager simply dies**. Reinforcements
   (`hurtServer` 449–671) are hard only, door breaking (`BreakDoorGoal`) hard only, the
   cave spider's poison normal and hard only (CaveSpider.txt 19–66). At Clustine's easy all
   four are absent, without draws.
8. **`Mob.doHurtTarget` in 26.3** (Mob.txt 3363–3438): the attack damage, the weapon's
   damage source, `EnchantmentHelper.modifyDamage` and the item's bonus, the hurt, and only
   if it took: `causeExtraKnockback` with `attack_knockback`, `hurtEnemy`,
   `doPostAttackEffects`, `setLastHurtMob`, the attack sound; then `postPiercingAttack`.
   Clustine's `melee_hit` (`goals.rs` 1623–1636) hurts with the attack damage and the default
   knockback only. And the swing: `MeleeAttackGoal` swings (`swingForAttack`, Mob.txt
   3570–3583), which sends `ClientboundSwingAnimationPacket` (n7d LivingEntity.txt 5139–);
   mobs have no swing state in Clustine, players have (`region.rs` 344–347, 2283–2289).

**Ranged attacks**

9. **A general "using an item" state is needed** (`LivingEntity.useItem`,
   `useItemRemaining`, ticked by `updatingUsingItem` before `aiStep`, n7d LivingEntity.txt
   9290–9315, 9472–9490, 9559–10003, with `DATA_LIVING_ENTITY_FLAGS` (8) bit 1 using and
   bit 2 the off hand): the skeleton draws its bow 20 ticks, the pillager charges its
   crossbow 25, the drowned raises its trident, and **a player draws a bow, charges a
   crossbow and raises a trident** — none of which Clustine has (`play.rs` 460: "Using items
   over time does not exist yet"; `RELEASE_USE_ITEM` is dropped). Today only the wandering
   trader and the witch keep a use of their own.
10. **Goal state survives `stop`** and belongs to the goal instance: the bow goal's strafing
    time and directions (RangedBowAttackGoal.txt 107–124), the crossbow goal's state,
    `attackDelay` and `updatePathDelay` (RangedCrossbowAttackGoal.txt 102–136), the spells'
    `nextAttackTickCount`. The game does not save them (a load starts fresh, **Inferred**);
    Clustine's goal state is in the mob's record, which is right for a restore.
11. **The rhythms at easy**: a skeleton's first arrow 20 ticks after it sees its target, then
    one every 60 (40 countdown + 20 draw; the bogged and the parched 90); a pillager charges
    25, waits `20 + nextInt(20)`, shoots on a tick it sees; a witch every 60; a drowned's
    trident every 40; a guardian's beam shows from tick 10 and strikes at 90 (the elder's at
    70); the evoker's fangs every 100 ticks of casting (a ring within 3 blocks, else a line
    of 16), its vexes every 340, its wololo every 140 (section 3).
12. **The wololo needs no target** (`Evoker$EvokerWololoSpellGoal.canUse` 27–97: no target,
    `mob_griefing`, a blue sheep within 16 that it sees, one own `nextInt(size)`): an evoker
    by blue sheep turns them red in ordinary play.
13. **The vex is a new type**, made by the evoker's summon and by its spawn egg: no physics,
    no gravity, its own move control, a life of `20 · (30 + nextInt(90))` ticks after which
    it is hurt by `starve` 1 every 20 ticks (Vex.txt 59–92), three goals that draw (section
    2.7).
14. **The guardian's beam is an entity id in synched data** (`ID_ATTACK_TARGET`, 17, set to
    the target's network id at `attackTime == 0`, Guardian$GuardianAttackGoal.txt 118–228):
    the edge must map it to each client's wire id as it maps a thrown item's owner today
    (`services/edge/src/fanout.rs` 6356–6369).

**Projectiles**

15. **`AbstractArrow`, `Arrow`, `SpectralArrow`, `ThrownTrident`, `ThrownEgg`, the potions and
    every projectile item (`BowItem`, `CrossbowItem`, `TridentItem`, `EggItem`, …) are not
    dumped** (section 14). What is dumped is the base the snowball uses (`Projectile`,
    `ThrowableProjectile`, `ThrowableItemProjectile`, `ProjectileUtil`,
    `ProjectileDeflection`) and the mobs' shots. The arrow's tick differs from the
    throwables' in ways that move it bit for bit (the move before the drag, gravity 0.05,
    water 0.6, the stuck test first, three draws to fall, a draw per hit sound, the critical
    `nextInt(j / 2 + 2)`, the piercing loop: **Inferred**); dump before building.
16. **An arrow's fate depends on whether its hurt took** (**Inferred**): taken, it is gone
    (or pierces on); refused (a creative player, invulnerability ticks), it reverses with one
    own draw, slows to 0.2 and may drop. Across a line the hurt is the holder's (A28 section
    7) and the sender cannot know the answer when it decides the arrow's fate.
17. **Item stacks carry no components** (`ItemStack { item, count }`; the protocol writes
    none and `untrusted_item_stack` discards a client's: `clustine-protocol/src/item.rs`).
    Tipped arrows, every potion, a crossbow's charge (`charged_projectiles`, which is how the
    client draws a loaded crossbow), fireworks, enchanted bows and the egg variants'
    defaults live in components. A creative player's tipped arrow or potion from the tab
    loses its contents at once. This is N9's largest shared-type decision.
18. **The dispenser shoots none of them today**: every projectile, the snowball included, is
    dropped as an item (`region/dispensers/behaviours.rs` 13–15). The game's
    `ProjectileDispenseBehavior` (n6, 36–103) serves arrows, eggs, snowballs, splash and
    lingering potions, experience bottles, fireworks, fire and wind charges; tridents and
    ender pearls are dropped.
19. **`make_thrown` and the snowball's tick generalise cheaply** to the egg, the potions, the
    experience bottle and the dispensed throwables (per-kind gravity, item and hit;
    `EntityData::Thrown` fits them, `region/thrown.rs`); the llama's spit is fully dumped
    (LlamaSpit.txt, Llama.txt 704–800) and Clustine's llama already "spits" a sound with
    nothing flying (`llama.rs` 633–642).

**Riding, effects and difficulty**

20. **A rider steers through its vehicle's navigation and move control** (`Mob.getNavigation`
    288–301 and `getMoveControl` 264–277 return the controlled vehicle's; look and jump
    stay the rider's), the vehicle's own move, jump and look goals stopped by
    `updateControlFlags` (598–640). The vehicle ticks first and follows what the rider
    wrote the tick before. In Clustine a unit lives in one chunk and one region and ticks
    vehicle first (`region/riding.rs` 798–855), so steering is a write-back within the
    region's tick, needing nothing at a line; but each mob's tick holds only its own record,
    and `update_control_flags` is hard-wired to "nothing" (`mobs/ai.rs` 739–766).
21. **Mob effects N9 reaches**: slowness (stray arrows, the witch), poison (bogged arrows, the
    witch), weakness (parched arrows, the witch), regeneration and instant health (a witch
    healing raiders), instant harming, glowing (spectral arrows), hunger (the husk's hit,
    nothing on a mob), wither (wither skeleton only). Clustine has the five N7k needed
    (`mobs/body/effects.rs`). The periodic ticks of poison, wither and regeneration are in
    subclasses not dumped (**Inferred**: `25 >> amp`, `40 >> amp`, `50 >> amp`).
22. **The effective difficulty Clustine passes is wrong**: `Spawning.difficulty` is `(0, 0)`
    everywhere (`mobs/types.rs` 1089–1095), right for the special multiplier at easy (always
    0) but not for the effective difficulty, which at easy lies in [0.75, 1.5]
    (DifficultyInstance.txt). N9's first readers of it (the zombie's fire on a hit, the
    husk's hunger: `140 · (int) eff` ticks) need it; it needs the clock, the moon and each
    chunk's inhabited time, which Clustine does not keep.

**Natural spawning**

23. **The tick's order** (ServerChunkCache.txt 794–961): build the spawn state (counts by
    category of every entity of a full loaded chunk, but persistent mobs and MISC); filter
    the categories once by the **global cap**; collect the spawning chunks (ticking, centre
    within 128 blocks horizontally of a non-spectator player); **shuffle them** with the
    level random (`Util.shuffle`, Util.txt 1957–1985: `nextInt(i)` for `i` from the size
    down to 2); then per chunk its inhabited time, thunder, and each category in enum order
    behind the **local cap**; then random ticks; then the custom spawners.
24. **The two caps**: global `max × spawnableChunks / 289` against the level's count
    (SpawnState.txt 164–183; monsters 70), the spawnable chunks being the 17×17 around every
    player the distance manager tracks, loaded or not, spectators included by default
    (**Inferred**, `DistanceManager` not dumped); local, per player, any player within 128
    blocks of the chunk with fewer than `max` of the category counted near them
    (`LocalMobCapCalculator`, not dumped, **Inferred**). The global filter is made once per
    tick, so a tick can overshoot.
25. **A chunk's attempt draws, in order**: x `nextInt(16)`, z `nextInt(16)`, y uniform from
    the bottom to the surface (caves included); 3 groups of `ceil(nextFloat() · 4)`
    attempts, each step `nextInt(6) − nextInt(6)` in x then z, the y kept; the type once per
    group; the type's rule draws (a monster `nextInt(32)` against raw sky light, then
    `nextInt(8)` against darkened light, `Monster.isDarkEnoughToSpawn`, Monster.txt 81–132;
    checked); the yaw `nextFloat() · 360` before the mob's own checks; then `finalizeSpawn`.
    At most four mobs of a category per chunk per tick.
26. **A type the game cannot make ends the whole position** (`getMobForSpawn` null →
    `spawnCategoryForPosition` returns, NaturalSpawner.txt 391): Clustine has **no slime and
    no enderman** (`mobs/types.rs` 1333–1398), which are 100 and 10 of the default monster
    list's 515; until they exist, a fifth of the overworld's monster positions are silenced.
27. **Structures replace the lists**: a swamp hut gives witches (and cats), a monument
    guardians, an outpost pillagers; ancient cities and trial chambers spawn nothing; the
    list is looked up again at every attempt (`canSpawnMobAt`, 680–690; ChunkGenerator.txt
    986–).
28. **Spawning and despawning need the nearest player anywhere**: the 24-block test from the
    nearest non-spectator player (3D) and from the world spawn, and `Mob.checkDespawn`'s
    distance (instant beyond 128, one in 800 beyond 32 once idle 600). The store already
    publishes every player's position once a second (`WorldPlayer`, `api.rs` 327–335; n7
    plan 9.2), but only sleep reads it (`region/sleep.rs` 455); `check_despawn`
    (`mobs/body.rs` 499–556) reads the region's players and the faces in its copies, so a
    monster near a line with its nearest player two chunks across may be discarded wrongly
    — today no monster spawns naturally, so nobody sees it; with N9 they will.

---------------------------------------------------------------------------------------

## 1. Targets

### 1.1 The shared parts (B 2.4 and 2.12, checked; what B does not say outright)

- `TargetGoal`, `HurtByTargetGoal`, `NearestAttackableTargetGoal`, `NonTameRandomTargetGoal`,
  `OwnerHurt*`, `ResetUniversalAngerTargetGoal` and `DefendVillageTargetGoal` are B 2.12
  (TargetGoal.txt 1–292, NearestAttackableTargetGoal.txt 1–205, HurtByTargetGoal.txt
  1–255); `TargetingConditions.test` and `getVisibilityPercent` (crouching × 0.8, invisible
  × 0.7 × armour cover, at least 0.1, a worn head's `MOB_VISIBILITY`) B 2.4.
- **Draws**: one own `nextInt(randomInterval)` per asked selector tick, **before** any
  search; `randomInterval = reducedTickDelay(interval)`, 5 for 10; an interval of 0 draws
  nothing (NearestAttackableTargetGoal.txt 77–99). The search box is
  `getBoundingBox().inflate(follow, follow, follow)` (101–111; Clustine's `goals.rs` ~1523
  matches); players by `getNearestPlayer(conditions, mob, x, eyeY, z)`.
- **HurtBy**: no draw; it starts on a new `lastHurtByMobTimestamp` with a `canAttack` that
  passes; `alertOthers` in `start` only (134–243): `getEntitiesOfClass(mob.getClass(),
  unitCube(position).inflate(follow, 10, follow), NO_SPECTATORS)` in the level's order,
  skipping itself, any with a target, another owner's tamed, allies, and the exact classes
  in `toIgnoreAlert` (compared by identity, 197–216); then `setTarget(attacker)`. No draws.
- **Who calls `setAlertOthers`**: the zombie (with `ZombifiedPiglin` ignored, Zombie.txt
  84–112), the drowned (the same, Drowned.txt 120–150, n7k), and `HurtByTargetGoal(Raider)`
  of the vindicator, evoker, vex and ravager. Not the witch, the pillager, the skeletons, the
  creeper or the spiders.

### 1.2 Each monster's target goals (priority: goal)

| Type | Targets | Notes |
|---|---|---|
| zombie, husk, zombie villager | 1 HurtBy alert (ZombifiedPiglin); 2 Player (must see); 3 AbstractVillager (no must see); 3 IronGolem (must see); 5 Turtle (10, must see, no reach, `BABY_ON_LAND_SELECTOR`) | Zombie.txt 89–122; the husk and zombie villager inherit (only `Zombie` and `Drowned` define `registerGoals`); the selector is `isBaby() && !isInWater()` (Turtle.txt 636–646) |
| drowned | 1 HurtBy(Drowned) alert; 2 Player (10, true, false, `okTarget`); 3 AbstractVillager; 3 IronGolem; 3 Axolotl (true, false); 5 baby turtles | `okTarget` = `!isBrightOutside() \|\| target.isInWater()` (Drowned.txt 508–524, n7k) |
| skeleton family | 1 HurtBy; 2 Player; 3 IronGolem; 3 Turtle babies | K 11.2; the wither skeleton adds `AbstractPiglin` at 3 (WitherSkeleton.txt 15–30, n7k) |
| creeper | 1 Player; 2 HurtBy (no alert) | K 11.3; so in Clustine only what hurt it |
| spider, cave spider | 1 HurtBy; 2 `SpiderTargetGoal(Player)`; 3 `SpiderTargetGoal(IronGolem)` | `canUse` false, **before** the interval draw, when the light at the eyes is worth 0.5 or more (Spider$SpiderTargetGoal.txt 53–78, n7k) |
| witch | 1 HurtBy(Raider), no alert; 2 `NearestHealableRaiderTargetGoal`; 3 `NearestAttackableWitchTargetGoal(Player, 10, true, false)` | the healer: `cooldown > 0` → false; own `nextBoolean()`; no raid → false (its file 38–66): one draw every asked tick outside raids. The attacker: `canAttack` (set by the witch's `aiStep` from the healer's cooldown, Witch.txt 181–230; always true outside raids) then the parent's `nextInt(5)` |
| vindicator | 1 HurtBy(Raider) alert; 2 Player; 3 AbstractVillager (**must see**); 3 IronGolem; 4 `VindicatorJohnnyAttackGoal` | Johnny: `NAT(LivingEntity, 0, true, true, attackable())`, `isJohnny` tested before the parent, so no draw (its file 1–45) |
| pillager | 1 HurtBy(Raider) alert; 2 Player; 3 AbstractVillager (no must see); 3 IronGolem | Pillager.txt 25–146 |
| evoker | 1 HurtBy(Raider) alert; 2 Player, unseen 300; 3 AbstractVillager (no must see), unseen 300; 3 IronGolem (no must see) | Evoker.txt 115–175 |
| vex | 1 HurtBy(Raider) alert; 2 `VexCopyOwnerTargetGoal`; 3 Player | the copy: `forNonCombat().ignoreLineOfSight().ignoreInvisibilityTesting()`, no draw (its file 1–75) |
| guardian, elder | `NAT(LivingEntity, 10, true, false, GuardianAttackSelector)` | the selector: a player, squid, glow squid or axolotl farther than 3 (14–35) |
| ravager | 2 HurtBy(Raider) alert; 3 Player; 4 AbstractVillager (adults, **Inferred** from the lambda at 926); 4 IronGolem | Ravager.txt 61–161; raids, eggs only |

In a creative world the Player goals still draw their `nextInt(5)` and find nobody.

---------------------------------------------------------------------------------------

## 2. Melee

### 2.1 `MeleeAttackGoal` and the hit (B 2.11, checked)

`canUse` at most every 20 game ticks, with no draw; the path's recalculation
`4 + nextInt(7)` (+10 beyond 32 blocks, +5 beyond 16, +15 when `moveTo` fails), and
`nextFloat() < 0.05f` drawn only when the target moved less than 1 and the countdown is 0;
the attack interval 20; `canPerformAttack` = cooldown 0, `isWithinMeleeAttackRange`, line of
sight. The reach: `getAttackBoundingBox` inflated `(r, 0, r)` with `DEFAULT_ATTACK_REACH =
sqrt(2.04f) − 0.6f`, **widened over a vehicle's box** in x and z (Mob.txt 3262–3361), which
matters for every jockey. `doHurtTarget` in order is finding 8; the swing is
`swingForAttack(MAIN_HAND)` with the item's attack animation (Mob.txt 3570–3583) →
`LivingEntity.swing(hand, anim, false)` → `swingState.startIfAble`, and if it started, the
swing packet to trackers (n7d LivingEntity.txt 5139–). `BreakDoorGoal` asks `isSwinging()`.

### 2.2 The zombie family

- **Goals** (Zombie.txt 89–197): 4 `ZombieAttackTurtleEggGoal(1.0, 3)` (K 11.8, built); 8
  look at players 8; 8 look around; then 2 `SpearUseGoal(1.0, 1.0, 10f, 2f)`; 3
  **`ZombieAttackGoal(1.0, false)`**; 6 `MoveThroughVillageGoal(1.0, true, 4,
  canBreakDoors)`; 7 stroll 1.0.
- **`ZombieAttackGoal`** (its file 1–69): `MeleeAttackGoal` with `raiseArmTicks`: 0 at
  start; each tick after the parent's, `raiseArmTicks++`, and the mob is aggressive (Mob's
  flags, data 15, the client raises its arms) iff `raiseArmTicks >= 5 && ticksUntilNextAttack
  < 10`; `stop` clears aggressive. No draws.
- **The drowned's `DrownedAttackGoal`** (n7k, 1–50): the same, its `canUse` and
  `canContinueToUse` also needing `okTarget`. Its trident goal at the same priority 2 comes
  first (section 3.6).
- **`doHurtTarget`** (Zombie.txt 672–711): the parent's; if it took, `eff =
  getCurrentDifficultyAt(blockPosition()).getEffectiveDifficulty()`; with an empty main hand,
  on fire, and own `nextFloat() < eff · 0.3f` → `target.igniteForSeconds(2 · (int) eff)`. The
  draw only when it took, empty-handed and burning — a burning zombie by day. At easy in a
  young world `(int) eff` is 0: the draw is made and nothing burns.
- **The husk's** (Husk.txt 38–77): empty-handed hit of a living target gives hunger for
  `140 · (int) eff` ticks, no draw; hunger does nothing to a mob (**Inferred**,
  `HungerMobEffect` not dumped).
- **`killedEntity`** (859–900): finding 7; the conversion (422–443, λ 1320–1369) is
  `villager.convertTo(ZOMBIE_VILLAGER, single(villager, true, true))`, then `finalizeSpawn`
  with `CONVERSION` and `ZombieGroupData(false, true)`, the villager data, gossips, offers
  and experience carried, level event 1026. Not reached at easy.
- **Reinforcements** (449–671), hard only and with an attacker (a creative player passes,
  since no target test is made): own `nextFloat() < spawn_reinforcements`, then up to 50
  tries of six own draws (`Mth.nextInt(r, 7, 40) · Mth.nextInt(r, −1, 1)` per axis), the
  placement and the type's rule (level draws), no living player within 7, unobstructed, the
  modifiers −0.05 on caller and callee. Not reached at easy.
- **Door breaking**: a zombie with `canBreakDoors` (a leader, K 3.4.1) holds `BreakDoorGoal`
  at 1 with a hard-only predicate; `DoorInteractGoal` (293 lines, dumped, not in B) finds a
  wooden door among the next path nodes within 1.5; the tick draws `nextInt(20) == 0` for
  level event 1019 and a swing; progress `(int)(breakTime / 240f · 10)` by
  `destroyBlockProgress`; at 240 the door goes with level events 1021 and 2001. Not reached
  at easy, but the goal is registered (the leader exists only above special 0, so at easy no
  zombie can break doors at all: K 3.4.1).
- **The spear** (`SpearUseGoal`, 390 lines): only with a kinetic weapon in hand and a
  target: a zombie's iron spear is one in 600 at easy (K 3.4.1), the husk on a camel husk's
  one in ten (K 3.4.2), common in deserts. Uses an item, flees by `LandRandomPos` with draws,
  pierces. **Inferred**: while it does not run, the holder falls through to
  `ZombieAttackGoal`.

### 2.3 The spiders (n7k)

- `SpiderAttackGoal` (1–51) = `MeleeAttackGoal(1.0, true)`, refusing to start as a vehicle
  (`isVehicle()`: a jockey's spider does not bite); `canContinueToUse` in light worth 0.5 or
  more draws own `nextInt(100)` each selector tick, and on 0 drops the target.
- `LeapAtTargetGoal(0.4f)` at 3: own `nextInt(reducedTickDelay(5))` (B 2.11), only with a
  target 2 to 4 away, on the ground.
- The cave spider's poison: easy none (section 5).
- The light is `getLightLevelDependentMagicValue` at (x, eyeY, z), 0 in an unloaded chunk
  (K 3.3), now N8's (`monster_ai.rs` 30–40).

### 2.4 The creeper (K 11.3, built)

`SwellGoal` is built (`creeper.rs` 115–151), the explosion too; what N9 adds is its melee
goal and targets: it walks at what hurt it (`MeleeAttackGoal(1.0, false)`, whose hit does
nothing), swells within 3 and explodes (radius 3, 6 charged). `AvoidEntityGoal` of cats and
ocelots runs without a target and is built.

### 2.5 The vindicator

Goals (Vindicator.txt 22–168, after `Raider.registerGoals`): 0 float; 1 avoid the creaking;
2 `VindicatorBreakDoorGoal` (needs a raid first, so no draw: 1–65); 3 `RaiderOpenDoorGoal`;
4 `Raider$HoldGroundAttackGoal(10f)` (patrols only: `canUse` needs no raid, patrolling, a
target, not aggressive, the last hurter not a player; `start` hands the target to every
raider within 8 that has none; `tick` beyond 10 draws `nextInt(50) == 0` for the ambient
sound); 5 `MeleeAttackGoal(1.0, false)`; 8 stroll 0.6; 9, 10 looks. `customServerAiStep`
sets `canOpenDoors` from `level.isRaided(pos)` (169–190; false without raids). The axe and
the hit are `Mob`'s.

### 2.6 The iron golem and the snow golem: what N9 changes

Nothing of their own (K 1.2–1.6): their targets now fight back, alert one another and seek
them. `before_push` (`iron_golem.rs` 543–553) already makes a pushing monster its target one
in 20.

### 2.7 The vex (Vex.txt and its inner classes; a new type)

- Body: health 14, attack 4, experience 3, `VexMoveControl` (19–35, 181–191); synched flags
  byte, bit 1 charging (192–203; index 16, **Inferred**); saved `bound_pos`, `life_ticks`,
  `owner` (205–275); `getLightLevelDependentMagicValue` 1.0 (401–404), so it never burns.
- `tick` (59–92): `noPhysics` true during the parent's tick, then false; no gravity; with a
  limited life, `--limitedLifeTicks <= 0` → 20 and `hurt(starve, 1)`.
- `finalizeSpawn` (406–426): an iron sword in the main hand (drop chance 0), the enchantment
  draws (level), then `Monster.finalizeSpawn` (K 5).
- Goals: 0 float; 4 `VexChargeAttackGoal` (MOVE, every tick: `canUse` needs a living
  target, no wanted position, own `nextInt(4) == 0`, then distance² > 4; `start` aims at the
  target's eyes at speed 1, charging on, `vex_charge`; `tick` hits when the boxes intersect,
  re-aims within 3); 8 `VexRandomMoveGoal` (`canUse` = no wanted position and own
  `nextInt(4) == 0`; `tick`: up to three tries of `origin.offset(nextInt(15) − 7,
  nextInt(11) − 5, nextInt(15) − 7)`, the first empty block's centre at 0.25); 9, 10 looks.
- `VexMoveControl.tick`: for MOVE_TO, `v = wanted − pos`; within the box's size WAIT and the
  motion halved; else `motion += v · (speed · 0.05 / |v|)`, the yaw from the motion (or the
  target), the body's yaw with it.

### 2.8 The ravager

Goals and targets dumped (Ravager.txt 61–161), its own `aiStep` (roar, stun, leaves broken,
296–540) and attack box (913). A raid's mob; a spawn egg makes one. Recommended later, with
raids (question 15.11).

---------------------------------------------------------------------------------------

## 3. Ranged and spells

### 3.1 Using an item (n7d LivingEntity.txt)

`LivingEntity.tick` runs `Entity.tick`, then `updatingUsingItem` (9290–9315: goes on only
while the same item is in the used hand; `updateUsingItem` 9472–9490 runs `onUseTick`, then
`--useItemRemaining`, `completeUsingItem` at 0 unless the item uses on release), then
`aiStep`. `startUsingItem` (9559–9628): nothing with an empty hand or while already using;
`useItem`, `useItemRemaining = getUseDuration` (bow and trident 72000, crossbow charge + 3:
**Inferred**); an equipment update; living flags (8) bit 1 on, bit 2 for the off hand; game
event `item_interact_start`. `getTicksUsingItem` (9912–9926) = duration − remaining.
`stopUsingItem` (9979–10003): flag off, `item_interact_finish` if it was using, empty, 0.
`releaseUsingItem` (9942–9977): the item's `releaseUsing`, then stop. A skeleton never
releases: it stops, so no arrow is spent and the bow neither wears nor sounds.

### 3.2 The skeletons (n7k)

- `AbstractSkeleton` (15–41): `bowGoal = RangedBowAttackGoal(this, 1.0, 20, 15f)`,
  `meleeGoal = AbstractSkeleton$1(this, 1.2, false)` (aggressive on start, off on stop).
- `reassessWeaponGoal` (269–327), server only: both removed; the stack in
  `getWeaponHoldingHand(BOW)` (the main hand if it holds a bow, else the off hand,
  ProjectileUtil.txt 702–712); a bow → `bowGoal` at 4 with the interval `HARD ? 20 : 40`
  (329–337; the bogged and the parched `HARD ? 50 : 70`, Bogged.txt 172–180, Parched.txt
  63–71); else `meleeGoal` at 4. Called from the constructor, `finalizeSpawn`, a load
  (450–457) and every server `onEquipItem` (459–472). Removing and adding puts the goal last
  in the selector's set (**Inferred**, GoalSelector.txt); Clustine registers it last
  (`skeletons.rs` 107–114).
- **`RangedBowAttackGoal`** (n7k): `attackTime = −1`, `strafingTime = −1`, radius² 225,
  MOVE|LOOK, ticking every tick (21–50, 126–129); `canUse` = a target and a bow in either
  hand (59–69); continue while `(canUse || !navigation.isDone()) && holding a bow` (79–95);
  `start` aggressive (97–105); `stop` (107–124) clears aggressive, `seeTime = 0`,
  `attackTime = −1`, stops using — and keeps the strafing fields. **`tick`** (131–384):
  1. no target → return; `d²`; `sees = sensing.hasLineOfSight(target)`;
  2. `seeTime` counts consecutive ticks seen (+) or unseen (−), reset to 0 when that flips;
  3. `d² > 225 || seeTime < 20` → `moveTo(target, 1.0)`, `strafingTime = −1`; else stop,
     `strafingTime++`;
  4. `strafingTime >= 20` → own `nextFloat() < 0.3` flips clockwise, own `nextFloat() < 0.3`
     flips backwards (**both always drawn**), `strafingTime = 0`;
  5. strafing: `d² > 168.75` → forwards, `d² < 56.25` → backwards; `moveControl.strafe(back ?
     −0.5 : 0.5, clockwise ? 0.5 : −0.5)`; a controlled `Mob` vehicle `lookAt(target, 30,
     30)`; then **`Mob.lookAt(target, 30, 30)`** (the instant body turn); not strafing:
     `lookControl.setLookAt(target, 30, 30)`;
  6. using: unseen and `seeTime < −60` → stop; seen and `t = getTicksUsingItem() >= 20` →
     stop, `performRangedAttack(target, BowItem.getPowerForTime(t))` (1.0 at 20,
     **Inferred**), `attackTime = interval`; not using: `--attackTime <= 0 && seeTime >= −60`
     → `startUsingItem(bow hand)`.
- **`performRangedAttack`** (339–423): the weapon; `getProjectile(weapon)` (Monster.txt
  214–: a held projectile in either hand, else an arrow; no draw); `getArrow` →
  `ProjectileUtil.getMobArrow` (714–739): `createArrow(level, ammo, mob, weapon)` then
  `setBaseDamageFromMob(power)` (**Inferred**: `power · 2 + triangle(diffId · 0.11,
  0.57425)`, two of the arrow's draws); `dx = tx − x`, `dy = target.getY(1/3) − arrow.y`, `dz
  = tz − z`, `h`; `spawnProjectileUsingShoot(arrow, …, dx, dy + h · 0.2f, dz, 1.6f,
  14 − 4 · diffId)` (10 at easy); the sound `skeleton_shoot` at 1.0 and `1 / (own nextFloat()
  · 0.4 + 0.8)` — drawn even when silent.
- Variants: the stray's arrow gains slowness 600 (Stray.txt 64–86), the bogged's poison 100
  (Bogged.txt 148–170), the parched's weakness 600 (Parched.txt 11–33; the parched cannot
  take weakness, 73–84), each only for an `Arrow`. The wither skeleton (melee; wither 200 on
  a hit; its arrows on fire 100 s; immune to wither: WitherSkeleton.txt 105–160) is not a
  Clustine type and not of the overworld.

### 3.3 The pillager

- Goals (Pillager.txt 25–146): 0 float; 1 avoid the creaking; 2 `HoldGroundAttackGoal(10f)`
  (patrols); **3 `RangedCrossbowAttackGoal(this, 1.0, 8f)`**; 8 stroll 0.6; 9 look at
  players 15; 10 look at mobs 15. `IS_CHARGING_CROSSBOW` at 17 (164–207); arm pose
  (230–250): charging, holding a crossbow, attacking, neutral.
- **`RangedCrossbowAttackGoal`** (n7k): state UNCHARGED, MOVE|LOOK, radius² 64,
  `PATHFINDING_DELAY_RANGE = UniformInt.of(20, 40)` (19–42, 370–376); **no `start`**, so it
  never sets aggressive; `canUse` a living target and a crossbow in either hand (44–55);
  `stop` (102–136): aggressive off, **`setTarget(null)`**, `seeTime = 0`, and if using: stop,
  charging off, then `getUseItem().set(CHARGED_PROJECTILES, EMPTY)` on the use item that
  `stopUsingItem` has just emptied (a no-op, the bytecode's order). State, `attackDelay` and
  `updatePathDelay` survive `stop`. **`tick`** (143–357): `seeTime` as the bow's; `move =
  (d² > 64 || seeTime < 5) && attackDelay == 0`; moving: `--updatePathDelay <= 0` →
  `moveTo(target, UNCHARGED ? 1.0 : 0.5)`, `updatePathDelay = own nextInt(21) + 20`
  (**Inferred**, UniformInt.txt); else `updatePathDelay = 0`, stop; `setLookAt(target, 30,
  30)`; then UNCHARGED and not moving → start using, CHARGING, charging on; CHARGING → back
  to UNCHARGED if not using, else at `getTicksUsingItem() >= getChargeDuration` (25,
  **Inferred**) **`releaseUsingItem()`** (the crossbow loads), CHARGED, `attackDelay = 20 +
  own nextInt(20)`, charging off; CHARGED → `--attackDelay == 0` → READY; READY and seen →
  `performRangedAttack(target, 1.0f)`, UNCHARGED.
- The shot (`CrossbowAttackMob.performCrossbowAttack`, not dumped, **Inferred**):
  `CrossbowItem.performShooting(…, 1.6, 14 − 4 · diffId, target)`; one arrow at angle 0 with
  base damage 2.0 (no mob triangle), crit off, the hit sound `crossbow_hit`; aimed by JOML
  float rotations; `shoot` (six draws); `crossbow_shoot` at pitch 1.0 for the first
  projectile; the crossbow wears by 1; `onCrossbowAttackPerformed` sets `noActionTime = 0`
  (208–214). The loading sounds at 20 % and 50 % may be flags of the item singleton, shared by
  every user (**Inferred**).

### 3.4 The witch (what attacking adds to K 3.6)

- Goal 2 `RangedAttackGoal(this, 1.0, 60, 10f)`. **`RangedAttackGoal`** (no draws; 23–256):
  `seeTime` ++ seen, 0 unseen; `d² > r² || seeTime < 5` → move, else stop; look; `--attackTime
  == 0` and seen → `f = √d / r`, `performRangedAttack(target, clamp(f, 0.1, 1))`,
  `attackTime = floor(f · (max − min) + min)`; `attackTime < 0` → `floor(lerp(√d / r, min,
  max))`. Clustine has it twice (`snow_golem.rs` 142–223, `llama.rs` 526–), each calling its
  own shot.
- **`performRangedAttack`** (Witch.txt 493–656): nothing while drinking; `v` the target's
  motion; `dx = tx + vx − x`, `dy = target.getEyeY() − 1.100000023841858 − y`, `dz = tz + vz −
  z`, `h`; the potion: a `Raider` target → healing at health ≤ 4 else regeneration, then
  `setTarget(null)`; else `h >= 8` and no slowness → slowness; else health ≥ 8 and no poison
  → poison; else `h <= 3`, no weakness and own `nextFloat() < 0.25` → weakness; else harming
  (the draw only when the three before it fail and the first two conditions of weakness
  hold); `spawnProjectileUsingShoot(ThrownSplashPotion::new, …, dx, dy + h · 0.2, dz, h > 2 ?
  0.75 : 0.45, 8.0)`; unless silent, `witch_throw` at `0.8 + own nextFloat() · 0.4` (drawn
  only when not silent).
- `aiStep`'s swiftness branch (a target farther than 11) becomes live: Clustine has
  `target_far = false` (`witch.rs` 112).
- Its own damage: 0 from its own potions; ×0.15 from `#witch_resistant_to` (452–491).

### 3.5 The evoker and its spells

- **`SpellcasterIllager`**: the spell byte at 17, `spellCastingTickCount` saved as
  `SpellTicks` (38–62, 293–), counted down in `customServerAiStep` (136–150); ids NONE 0,
  SUMMON_VEX 1, FANGS 2, WOLOLO 3, DISAPPEAR 4, BLINDNESS 5. `SpellcasterCastingSpellGoal`
  (MOVE|LOOK while casting, stops the navigation, looks at the target or the wololo sheep).
  `SpellcasterUseSpellGoal` (21–131): no flags; `canUse` a living target, not casting,
  `tickCount >= nextAttackTickCount`; `start`: warmup `adjustedTickDelay(20)`, casting
  ticks, `next = tickCount + interval`, the prepare sound (no draw), the spell; `tick`:
  `--warmup == 0` → `performSpellCasting`, `evoker_cast_spell`.
- Goals (Evoker.txt 16–166): 0 float; 1 casting; 2 avoid players (8, 0.6, 1.0); 3 avoid the
  creaking; 4 summon; 5 fangs; 6 wololo; 8 stroll 0.6; 9, 10 looks. Speed 0.5, follow 12,
  health 24.
- **Fangs** (`EvokerAttackSpellGoal`, 28–317): casting 40, interval 100. `yaw =
  atan2(tz − z, tx − x)`, `minY = min(ty, y)`, `maxY = max(ty, y) + 1`; within 3 blocks
  (`d² < 9`) five fangs at `yaw + i · π · 0.4f` radius 1.5 warmup 0, then eight at `yaw + i ·
  π · 2 / 8 + 1.2566371f` radius 2.5 warmup 3; otherwise 16 in a line at `1.25 · (i + 1)`,
  warmup `i`. No draws. Each by `createSpellEntity`: from `(x, maxY, z)` down to `floor(minY)
  − 1`, the first cell whose block below is sturdy on top, raised by its own collision
  shape's top; `EvokerFangs` there and game event `entity_place`.
- **`EvokerFangs`** (not dumped, **Inferred**): 0.5 × 0.8; at warmup −8 it hurts every living
  entity in its box inflated (0.2, 0, 0.2) but its owner and allies by `indirectMagic` 6;
  entity event 4 once; gone after 22 ticks of life. No server draws.
- **Summon** (`EvokerSummonSpellGoal`, 27–175): casting 100, interval 340; `canUse` the
  parent's, then count vexes within 16, then own `nextInt(8) + 1 > count`; three times:
  `offset(−2 + nextInt(5), 1, −2 + nextInt(5))`, `VEX.create(MOB_SUMMONED)`, `finalizeSpawn`
  (level draws), owner, bound origin, **`setLimitedLife(20 · (30 + own nextInt(90)))`**,
  `addFreshEntityWithPassengers`, `entity_place`.
- **Wololo** (`EvokerWololoSpellGoal`): finding 12; warmup 40, casting 60, interval 140; the
  sheep's colour set to red.
- The illusioner has no natural spawn and no egg: left out.

### 3.6 The guardians (K 3.8 for their bodies)

- **`GuardianAttackGoal`** (113–228): every tick; `canUse` a living target;
  `canContinueToUse` that and (the elder, or `d² > 9`); `start`: `attackTime = −10`,
  navigation stopped, look (90, 90); `stop`: beam 0, target null, the stroll triggered;
  `tick`: stop, look; unseen → target null; `++attackTime == 0` → the beam's id and entity
  event 21 unless silent; `attackTime >= 80` (the elder 60, ElderGuardian.txt 46–) →
  `hurtServer(indirectMagic(g, g), 1 (+2 hard) (+2 elder))`, then `doHurtTarget` (melee by
  its attack damage), target null. No draws.
- `Guardian.aiStep` turns its yaw to the head's while attacking (726–739); its water
  travel's sink stops with a target (K).
- **The elder's curse** (ElderGuardian.txt 86–127; MobEffectUtil.txt 112–134): every 1200
  ticks mining fatigue III for 6000 ticks to **survival** players within 50 — never a
  creative player; built (`elder_guardian.rs` 24–86).

### 3.7 The drowned's trident (n7k)

- Goal 2 `DrownedTridentAttackGoal(this, 1.0, 40, 10f)` before `DrownedAttackGoal`, also
  at 2: the parent `RangedAttackGoal` plus a trident in the main hand; `start` adds
  aggressive and **`startUsingItem(MAIN_HAND)`** (only the raised trident shows; never
  released); `stop` stops using.
- `performRangedAttack` (657–747): the held trident or a new one; `new ThrownTrident(level,
  this, stack)`; the skeleton's aim; `spawnProjectileUsingShoot(…, 1.6f, uncertainty)` (the
  field −1 means the interface's, 749–762); `drowned_shoot` at `1 / (own nextFloat() · 0.4 +
  0.8)`. It keeps its trident.

### 3.8 The llama's spit (finished in passing)

`Llama.spit` (Llama.txt 704–800): `new LlamaSpit(level, llama)` at `x − (w + 1) · 0.5 ·
sin(bodyYaw°)`, `eyeY − 0.1`, `z + (w + 1) · 0.5 · cos(bodyYaw°)`; aim at `getY(1/3)`, `+ h ·
0.2`; `spawnProjectileUsingShoot(…, 1.5, 10)`; `llama_spit` at `1 + (r1 − r2) · 0.2` (two own
draws). `LlamaSpit.tick` (69–140): `Projectile.tick`, the hit along the motion, then the new
position, rotation, gone when no block in its box is air or in water, motion × 0.99, gravity
0.06, `setPos`; a hit hurts by `spit` 1.0 (146–189); a block hit discards it. No synched
data.

---------------------------------------------------------------------------------------

## 4. Projectiles

### 4.1 The base (dumped, n7k; the snowball built on it)

- **`Projectile`**: first tick the `projectile_shoot` game event and `hasBeenShot`;
  `checkLeftOwner` (box expanded by the motion and inflated by 1 meets none of the owner's
  unit) (197–262). `getMovementToShoot` (263–301): normalise, add `triangle(0, 0.0172275 ·
  u)` per axis (x, y, z: **six `nextDouble`** of the projectile's own random), × power.
  `shoot` sets yaw `atan2(x, z) · 57.2957763671875`, pitch likewise (302–349).
  `shootFromRotation` (350–416) adds the shooter's known movement, its y only when not on the
  ground. `spawnProjectileUsingShoot` (515–547): create (the UUID's draws), shoot (six),
  add, `applyOnProjectileSpawned` (enchantments; nothing without).
- **`hitTargetOrDeflectSelf`** (613–692): an entity's `deflection` — `REVERSE` only for
  `#deflects_projectiles` (the breeze; n7 Entity.txt 10141–10150), so `NONE` in Clustine —
  then `onHit`: the `projectile_land` game event, `onHitEntity` or `onHitBlock`
  (`state.onProjectileHit`: a target block's signal, a bell, a chorus flower, a pointed
  dripstone, TNT lit by a burning arrow, a decorated pot — **the blocks' hooks are Clustine's
  "waits for N9"** in `region/thrown.rs` 24–28).
- **`ProjectileDeflection.REVERSE`** (48–92): one own `nextFloat()`, yaw `+= 170 + 20 f`,
  motion `× −0.5 · scale`.
- **`ProjectileUtil.getHitResultOnMoveVector`** (11–31, 125–169) and `getEntityHitResult`
  (378–443) with `computeMargin` (`max(0, min(0.3, (tickCount − 2) / 20))`, 363–377): built
  (`region/thrown.rs`). `getManyEntityHitResult` is the spear's, not the arrow's.
- `isPickable` only in `#redirectable_projectile` (1024): arrows, snowballs and potions never
  hit one another. The add packet's data is the owner's network id (933–968): built.
- **`ServerEntity.sendChanges`** (526–549): an `AbstractArrow` always gets a move packet with
  position and rotation, whatever the delta.

### 4.2 Arrows (not dumped; **Inferred**, constants confirmed by Pumpkin)

- **Construction**: at the owner's eye − 0.1; pickup ALLOWED for a player owner, DISALLOWED
  for a mob; CREATIVE_ONLY when the bow drew intangible ammo, which a creative player's bow
  does (`useAmmo` with `hasInfiniteMaterials`); base damage 2.0; pierce from the weapon's
  enchantment.
- **Synched** (Pumpkin's extraction): flags byte 8 (crit 1, no physics 2), pierce byte 9,
  in-ground boolean 10; `Arrow`'s colour int 11 (−1); the trident's loyalty byte 11 and foil
  boolean 12. The living entity's arrow count is int 12 (n7d LivingEntity.txt 411); in
  `tick` (7417–7458) with a count, `removeArrowTime = 20 · (30 − count)` when it runs out,
  then down by one, and at 0 one arrow less. No draw.
- **`tick`**: the motion `v` first; in the ground if the block at its position has a
  collision box containing it; `shakeTime--`; in water or rain the fire is put out; **in the
  ground**: if the block changed and `shouldFall` (no collision in its box inflated by
  0.06), it falls — motion × `(nextFloat · 0.2, nextFloat · 0.2, nextFloat · 0.2)` (**three
  own draws**), life 0; else `++life >= 1200` discards it; **flying**: in water × 0.6 (the
  trident 0.99, Pumpkin says 0.9: dump), rotation lerps, clip with the blocks and
  `stepMoveAndHit` (the entity hit along the path, `setPos` at the hit, the block hit if no
  entity, looping on while it pierces), **then** × 0.99 unless in water, **then** gravity
  0.05. The drag after the move is the reverse of the throwables'.
- **`onHitEntity`**: `j = ceil(clamp(|motion| · damage, 0, MAX_INT))`; a crit adds own
  `nextInt(j / 2 + 2)`; the owner's `setLastHurtMob`; on fire → the target alight 5 s (not
  an enderman); if the hurt (`arrow(this, owner)`) took: a living target's arrow count + 1,
  Punch knockback, `doPostHurtEffects` (a tipped or mob arrow's effects; spectral: glowing
  200), the sound `arrow_hit` at `1.2 / (own nextFloat() · 0.2 + 0.9)`, discarded unless it
  pierces; **if refused**: `deflect(REVERSE)` (one own draw), motion × 0.2, and if nearly
  still, dropped as its item (when ALLOWED) and discarded.
- **`onHitBlock`**: position = hit − 0.05 · sign(motion), motion zero, the ground sound
  (one own draw), in the ground, shake 7, crit and pierce off.
- **Pick-up** (`playerTouch`): in the ground (or no physics) and shake 0; ALLOWED adds the
  item, CREATIVE_ONLY needs infinite materials; `take(entity, 1)` sends
  `TakeItemEntity` (n7d LivingEntity.txt 8992–9010). So **a creative player's arrows can be
  picked up by them, and add nothing**.
- An arrow or a trident **kills an armour stand outright** (`#always_kills_armor_stands`,
  `clustine-data` `tags.rs` 5184); today's thrown hit ignores stands.

### 4.3 Tridents (not dumped; **Inferred**)

Damage 8 (`trident(this, owner)`); on a hit `dealtDamage`, motion × (−0.01, −0.1, −0.01),
`trident_hit`; no arrow count; it hits nothing more once it has dealt damage, nor after 4
ticks in the ground; loyalty (an enchantment: a component) brings it back to its owner;
picked up only by its owner or when it has none; a drowned's is DISALLOWED and stays 1200
ticks. A player's: released after at least 10 ticks of use, `consumeAndReturn` (a creative
player keeps theirs), shot at (0, 2.5, 1.0), CREATIVE_ONLY for a creative player, then
`trident_throw` with no draw. Riptide (an enchantment) is out with components.

### 4.4 Thrown items (eggs, potions, bottles, pearls)

Each reuses the snowball's flight (`ThrowableItemProjectile`, eye − 0.1, `DATA_ITEM_STACK`
8; ThrowableItemProjectile.txt 30–40), with its gravity and hit (**Inferred**, Unverified
(SteelMC/Pumpkin) for the constants):

| Item | Use (roll, power, uncertainty; sound from the level random L, 0.5, `0.4 / (L·0.4 + 0.8)`) | Gravity | Hit |
|---|---|---|---|
| egg (and blue, brown) | 0, 1.5, 1.0; `egg_throw`, players | 0.03 | `thrown` 0 to an entity; own `nextInt(8) == 0`, then own `nextInt(32) == 0` for four; each chick `CHICKEN.create(TRIGGERED)`, age −24000, at the egg, the variant from the stack's component; entity event 3; gone |
| splash potion | −20, 0.5, 1.0; players | 0.05 | water: the water-sensitive hurt, fire put out; effects: every living entity in `inflate(4, 2, 4)` with d² < 16, `prox = 1 − √d / 4`, instant effects scaled, others `(int)(prox · dur · scale + 0.5)` if above 20; level event 2007 (instant) or 2002 with the colour; gone |
| lingering potion | −20, 0.5, 1.0; neutral | 0.05 | an `AreaEffectCloud` (radius 3, −0.5 a use, 600 ticks, wait 10): a whole entity type |
| experience bottle | −20, 0.7, 1.0; neutral | 0.07 | level event 2002; orbs `3 + nextInt(5) + nextInt(5)` |
| ender pearl | 0, 1.5, 1.0; neutral; 20 ticks' cooldown | 0.03 | the owner teleported, an endermite one in 20 (**Inferred**); a saved pearl (`ServerPlayer`) |

### 4.5 Bows and crossbows in a player's hand (not dumped; **Inferred**)

- **Bow**: use starts when `getProjectile` is not empty — always an arrow for a creative
  player (n7d Player.txt 4488–4563); on release `f = t / 20, f = (f² + 2f) / 3`, capped at 1,
  nothing below 0.1 (three ticks at least); `shootFromRotation(player, xRot, yRot, 0, 3f ·
  f, 1.0)`, crit at full power; after the spawn `arrow_shoot` at `1 / (L · 0.4 + 1.2) + f ·
  0.5`. Draws: the arrow's 2 + 6, then L 1.
- **Crossbow**: charging 25 ticks with its two loading sounds; at release with full power
  the projectiles stored in `charged_projectiles` (a component) and the end sound; the next
  use shoots at 3.15 (a firework 1.6), uncertainty 1.0, crit for a player, one per stored
  projectile at 0, −10, +10 (multishot); `crossbow_shoot` at `getShotPitch` (1.0 for the
  first, else `1 / (R · 0.5 + 1.8) + 0.63 or 0.43`, from the shooter's random).
- **The serverbound side**: `ServerboundUseItem` starts a use; `ServerboundPlayerAction`
  `RELEASE_USE_ITEM` (6) → `player.releaseUsingItem()` (n7d
  ServerGamePacketListenerImpl.txt 3962). Other viewers see the drawn bow by the living
  flags (8).

### 4.6 Dispensers (n6)

`ProjectileDispenseBehavior.execute` (36–84): the position `config.positionFunction` (the
dispenser's centre + 0.7 · facing, DispenserBlock.txt 304–348), `asProjectile` (the UUID's
draws), `spawnProjectileUsingShoot(…, facing step, config.power, config.uncertainty)` (six),
`stack.shrink(1)`; the sound level event `overrideDispenseEvent` or 1002 (86–103). The
config's defaults are in `DispenseConfig$Builder`, not dumped (**Inferred**: uncertainty
6.0, power 1.1; potions and bottles 3.0 and 1.375; fireworks 1.0, 0.5, event 1004). An arrow
from a dispenser is ALLOWED with no owner.

### 4.7 What crosses a line of a projectile

A projectile is an entity: it crosses as one (A28 section 5; the snowball's `relocate`),
and its hit on what only a copy shows is an entity effect (the snowball's `EntityUse::Thrown`,
tag 10). Section 11 has the rest.

---------------------------------------------------------------------------------------

## 5. Mob effects

- **`addEffect`** (n7d LivingEntity.txt 2311–2371): `canBeAffected`, then put new (with
  `onEffectAdded`) or `update` an existing one; no game-mode test. **`canBeAffected`**
  (2372–2416): `#immune_to_infested`, `#immune_to_oozing`, and `#ignores_poison_and_regen`
  (the undead) refusing poison and regeneration; overrides: spider and cave spider refuse
  poison (Spider.txt 201–215, n7k), the wither skeleton wither, the parched weakness, the
  nautilus poison. Clustine names that it asks no type (`effects.rs` 27–28).
- **`tickEffects`** (1867–1998): each effect's `tickServer` (MobEffectInstance.txt 456–500):
  `d` = the remaining duration (or `tickCount` for an infinite one) **before** the countdown;
  if `shouldApplyEffectTickThisTick(d, amp)` and `applyEffectTick` returns false, the effect
  is removed; then the countdown and the hidden effect. No server draws. Clustine's
  `tick_effects` reads `remains(now − 1)`, which is the game's `d`.
- **The subclasses (not dumped, Inferred)**: poison every `25 >> amp` ticks hurts 1 (magic)
  while health > 1; wither every `40 >> amp` hurts 1 (wither); regeneration every `50 >> amp`
  heals 1; hunger every tick, exhaustion on a player, nothing on a mob; instant health and
  harming heal `4 << amp` or hurt `6 << amp` (magic), inverted for the undead, a splash
  scaling both by its proximity (`(int)(f · v + 0.5)`).
- **Modifiers** (MobEffects.txt, × (amp + 1)): slowness −0.15 speed (multiplied total,
  142); weakness −4 attack damage (add, 312); mining fatigue −0.1 attack speed (170);
  strength +3 (184). Glowing (378) sets the shared flag bit 6 (`updateGlowingStatus`,
  2063–2076).
- **What reaches whom in N9**: a cave spider's poison: never at easy. A husk's hunger: a
  mob, nothing. The witch's potions and the skeletons' arrows: mobs; **a creative player
  only by a splash beside them** (finding 3), which Clustine cannot show (players have no
  effects at all: `elder_guardian.rs` 1–7, `living.rs`).
- **Particles**: `DATA_EFFECT_PARTICLES` is not sent by Clustine (named in `effects.rs`); a
  poisoned golem would show no swirls.

---------------------------------------------------------------------------------------

## 6. Mobs riding mobs, steered by their riders

- **Who steers** (Mob.txt 306–323): the first passenger, if the vehicle is not `NoAI`, the
  passenger is a `Mob` and `canControlVehicle()` (not `#non_controlling_rider`: slime,
  magma cube, sulfur cube; n7 Entity.txt 7610–7625). A saddled horse or nautilus with a
  player first is the player's (AbstractHorse.txt 2420–2437, AbstractNautilus.txt
  263–282); an unsaddled trap skeleton horse is its skeleton's, a zombie nautilus its
  drowned's. `getControlledVehicle` (Entity.txt 10485–10510): only the first passenger.
- **What is redirected**: `getNavigation()` and `getMoveControl()` return the controlled
  vehicle's (Mob.txt 264–301); `getLookControl()` and `getJumpControl()` do not. The ticks
  use the fields: `serverAiStep` (1705–1815) runs sensing, the target selector, the goal
  selector, `navigation.tick()`, `customServerAiStep`, then move, look and jump controls —
  the vehicle's own. So the path is planned with the vehicle's position, size, node
  evaluator and follow range, and its speed is the rider's modifier × the vehicle's speed.
- **`updateControlFlags`** (598–640), every fifth tick from `Mob.tick`: `notMob =
  !(getControllingPassenger() instanceof Mob)`, `notBoat = !(getVehicle() instanceof
  AbstractBoat)`; MOVE = notMob, JUMP = notMob && notBoat, LOOK = notMob. The vehicle keeps
  its target selector. The ravager also counts a `#raiders` controller as not a mob
  (Ravager.txt 162–200). A brain vehicle (the zombie nautilus, ZombieNautilus.txt 47–72)
  keeps driving its own navigation from its brain; both write it (**Inferred**).
- **Travel**: `travelRidden` only for a player controller (n7d LivingEntity.txt
  7051–7090); a mob-steered vehicle travels by its own move control.
- **Order** (n7d ServerLevel.txt 1698–1800; Entity.txt 6785–6800): the root's tick, then each
  passenger's `rideTick` (motion zeroed, `tick()`, `positionRider`), depth first. **The
  vehicle moves first, by what its rider wrote the tick before; the rider then writes
  again.**
- **What a rider's goal writes into the vehicle**: its navigation (`createPath`, `moveTo`,
  `stop`, `isDone`, `getPath`, read back at once in the same goal: `MeleeAttackGoal.canUse`
  takes `createPath(target, 0) != null`); its move control (`setWantedPosition`,
  `strafe`); and with the bow goal its rotation (`controlledVehicle.lookAt(target, 30, 30)`,
  RangedBowAttackGoal.txt 300–312). Nothing else.
- **Body turn**: `AbstractSkeleton.rideTick` (166–181, n7k) and `Drowned.rideTick` (789–,
  n7k) copy the vehicle's body yaw when it is a `PathfinderMob`; the zombie does not.
- **The jockeys**: a chicken jockey's chicken stops its panic, tempt and stroll and is driven
  by the baby zombie's attack goal; a spider jockey's spider keeps targeting but loses its
  attack and leap, the skeleton paths and strafes it by its wall-climbing navigation and
  turns it; a trap skeleton horse is driven by its skeleton's bow goal; a zombie nautilus
  by both its brain and its drowned.
- **The skeleton trap** (SkeletonTrapGoal.txt, 279 lines): only from `tickThunder`
  (raining at the place, `nextDouble() < eff · 0.01`, n7d ServerLevel.txt ~905–945) —
  Clustine has no weather; its horsemen are enchanted by the providers. Left with weather.

---------------------------------------------------------------------------------------

## 7. The difficulty

- **`DifficultyInstance`** (whole file): peaceful 0; otherwise `eff = id · (0.75 + t + c')`,
  `t = clamp((clock − 72000) / 1440000, 0, 1) · 0.25`, `c = clamp(inhabited / 3600000, 0, 1) ·
  (hard ? 1 : 0.75) + clamp(moon · 0.25, 0, t)`, `c' = easy ? c · 0.5 : c`. Special
  multiplier: 0 below 2, 1 above 4, else `(eff − 2) / 2`. At easy `eff` is in [0.75, 1.5],
  the special multiplier always 0; `(int) eff` is 0 until about a million clock ticks.
- **`getCurrentDifficultyAt`** (n7d ServerLevel.txt 1311–): the chunk's inhabited time and the
  moon's brightness where it is loaded, 0 and 0 otherwise. Inhabited time rises by one each
  tick a chunk is a spawning chunk (ServerChunkCache.txt 929–935).
- **Where N9 reads it**: the level's difficulty — uncertainty `14 − 4 · id`
  (RangedAttackMob.txt, n7k), the skeleton's interval, the guardian's damage, the cave
  spider's poison, reinforcements, door breaking, conversion, peaceful's despawning and
  spawning refusal (SpawnPlacements.txt 77–100; Mob.txt 1605–1630); the effective — the
  zombie's fire, the husk's hunger, the trap's chance; the special multiplier — every
  `finalizeSpawn` (0 at easy, K 0.4).
- **Clustine**: `Difficulty::WORLD = Easy`, a constant (`clustine-world/src/difficulty.rs`;
  P 11 item 5); readers `navigation.rs` 112, `zombie_family.rs` 261, `skeletons.rs` 129,
  `spider.rs` 204, `random_ticks/fire.rs` 41; no inhabited time anywhere; `moon_brightness`
  exists (`mobs/making.rs`).

---------------------------------------------------------------------------------------

## 8. Natural spawning

### 8.1 Where it runs, its switches and its order

- `ServerLevel.tick` (n7d 426–670): border, weather, sleep, sky, time; block and fluid ticks;
  raids; **`chunkSource.tick`** (spawning); block events; entities (`checkDespawn`, then the
  tick); block entities. `ServerChunkCache.tick` (685–727) → `tickChunks` (794–928), finding 23.
- Switches: game rules `spawn_mobs` and `spawn_monsters` (both true by default;
  `isSpawningMonsters` = both, n7d ServerLevel.txt 4187–4205; n7d GameRules.txt 673–761 with
  `spawn_patrols`, `spawn_phantoms`, `spawn_wandering_traders`, `spawn_wardens`,
  `spectators_generate_chunks`); `spawnEnemies` set by `Level.setSpawnSettings` from the
  server (**Inferred**: from `isSpawningMonsters` and not peaceful). Creatures (the
  persistent category) only when `gameTime % 400 == 0` (838–840). Clustine models none of
  these rules: they are on.
- **`getFilteredSpawningCategories`** (NaturalSpawner.txt 92–135): categories in enum order
  without MISC (MONSTER, CREATURE, AMBIENT, AXOLOTLS, UNDERGROUND_WATER_CREATURE,
  WATER_CREATURE, WATER_AMBIENT), each kept if `(spawnEnemies || friendly) && (persistent tick
  || !persistent) && canSpawnForCategoryGlobal`.
- **`tickSpawningChunk`** (929–961): `incrementInhabitedTime()`; in entity-ticking range
  `tickThunder` (draws only in a thunderstorm); nothing more without categories; if
  `canSpawnEntitiesInChunk` (`entityManager.canPositionTick` and inside the border),
  `spawnForChunk`. So each chunk's thunder draws fall between its neighbours' spawning draws.

### 8.2 The state and the caps

- **`createState`** (23–90, λ 1182–1217): every entity of `level.getAllEntities()` but a
  persistent mob (`isPersistenceRequired` or `requiresCustomPersistence`: a passenger or a
  leashed mob by default, Mob.txt 1592–1603) and MISC; only if its chunk is a full loaded
  chunk (ServerChunkCache.txt 963–980): the biome's spawn cost charged to the potential, the
  local cap's `addMob(chunk, category)`, the count + 1.
- **Caps and despawn distances** (MobCategory.txt 166–263):

  | category | max | friendly | persistent | despawn | no despawn |
  |---|---|---|---|---|---|
  | MONSTER | 70 | no | no | 128 | 32 |
  | CREATURE | 10 | yes | yes | 128 | 32 |
  | AMBIENT | 15 | yes | no | 128 | 32 |
  | AXOLOTLS | 5 | yes | no | 128 | 32 |
  | UNDERGROUND_WATER_CREATURE | 5 | yes | no | 128 | 32 |
  | WATER_CREATURE | 5 | yes | no | 128 | 32 |
  | WATER_AMBIENT | 20 | yes | no | 64 | 32 |
  | MISC | −1 | yes | yes | 128 | 32 |

- **Global** (SpawnState.txt 164–183): `count < max · spawnable / 289` in integers
  (`MAGIC_NUMBER = 17²`, NaturalSpawner.txt 1244–1248). The spawnable count is
  `DistanceManager.getNaturalSpawnChunkCount()` (not dumped; **Inferred**: the chunks within
  Chebyshev 8 of a tracked player, overlaps once, loaded or not; players are tracked unless
  `isSpectator && !spectators_generate_chunks`, ChunkMap.txt 2346–2420).
- **Local** (185–198): `LocalMobCapCalculator.canSpawn(category, chunk)` (not dumped;
  **Inferred**: true if any player whose 128 blocks reach the chunk's centre has fewer than
  `max` of the category counted in the chunks near them; `addMob` raises it for every such
  player). Checked per chunk and category before the position's draws: a refusal costs none.
- **The potential** (`PotentialCalculator`, not dumped): only where a biome has spawn costs,
  which no overworld biome has (soul sand valley, warped forest): left for the Nether.
- **The spawning chunks** (ChunkMap.txt 2105–2139, 2203–2344): a holder with a ticking chunk
  and a non-spectator player whose horizontal distance to the chunk's centre is below 128
  (`(cx·16 + 8 − px)² + (cz·16 + 8 − pz)² < 16384`).

### 8.3 One chunk (NaturalSpawner.txt 137–819)

1. Per category in order, behind the local cap: `getRandomPosWithin` (743–785): `x = minX +
   nextInt(16)`, `z = minZ + nextInt(16)`, `top = chunk.getHeight(WORLD_SURFACE, x, z) + 1`,
   `y = nextInt(top − minY + 1) + minY` (**Inferred**: `getHeight` is the heightmap's first
   free cell − 1, so `top` is Clustine's stored value); `y < minY + 1` → nothing.
2. `spawnCategoryForPosition` (215–443): a redstone conductor at the position → nothing (no
   draw). Then **three groups**, each from the position's x and z with the same y, a fresh
   type and group data, `packSize = ceil(nextFloat() · 4)`; each attempt up to `packSize`:
   1. `x += nextInt(6) − nextInt(6)`, `z += nextInt(6) − nextInt(6)` (four draws);
   2. the nearest non-spectator player at any distance (`getNearestPlayer(…, −1, false)`, n5
      EntityGetter.txt 212–226), none → next attempt; `d2` 3D to `(x + .5, y, z + .5)`;
   3. `isRightDistanceToPlayerAndSpawnPoint` (445–499): `d2 > 576`, not within 24 of the
      world's respawn point (in this dimension), and the block's chunk this one or
      `canSpawnEntitiesInChunk(it)`;
   4. the type, once per group: `getRandomSpawnMobAt` (650–678; for WATER_AMBIENT in
      `reduced_water_ambient_spawns`, `nextFloat() < 0.98` → none) → `mobsAt(…).getRandom`
      (one `nextInt(total)`, n8 WeightedList.txt 149–166); none → **the group ends**; else
      `packSize = count.sample` (a uniform count draws);
   5. `isValidSpawnPostitionForType` (501–579): not MISC; within the category's despawn
      distance unless the type `canSpawnFarFromPlayer`; `canSpawnMobAt` (the list at this
      block still has the entry); `SpawnPlacements.isSpawnPositionOk`;
      `checkSpawnRules(type, level, NATURAL, pos, level.random)` (**the type's draws**);
      `noCollision(type.getSpawnAABB(…))` (entities and the border too, **Inferred**);
   6. the potential (none in the overworld);
   7. `type.create(level, NATURAL)`; **null → the whole call returns** (391);
   8. `snapTo(x + .5, y, z + .5, nextFloat() · 360, 0)` — the yaw drawn here;
   9. `isValidPositionForMob` (615–648): beyond the despawn distance and `removeWhenFarAway`
      → no; `mob.checkSpawnRules(level, NATURAL)` (a `PathfinderMob`: walk target value ≥ 0;
      a monster's is `0.5 − magic` with darkness, Monster.txt 73–79, so raw darkened light ≤
      12); `checkSpawnObstruction` (`!containsAnyLiquid && isUnobstructed`, Mob.txt
      2008–2022; the drowned and guardian only `isUnobstructed`);
   10. `finalizeSpawn(level, difficultyAt(mob), NATURAL, groupData)` (level draws,
       **Inferred** that `Mob.finalizeSpawn` uses `level.getRandom()`);
   11. `addFreshEntityWithPassengers`, `afterSpawn` (the counts and the local cap);
   12. the total across groups ≥ `getMaxSpawnClusterSize()` (4; the pillager 1, Pillager.txt
       269–272) → return; `isMaxGroupSizeReached` (only the tropical fish) → the group ends.
3. **`mobsAt`** (692–708): a monster on nether bricks in a fortress gets the fortress's list;
   otherwise each structure at the block, its `spawnOverrides[category]` (`piece`: a piece
   at the block; `full`: the start's box), the first that matches, else the biome's
   `gameplay/natural_mob_spawns` attribute (the lists are environment attributes in 26.3).

| Structure | Box | Override |
|---|---|---|
| swamp hut | piece | monster: witch ×1; creature: cat ×1 |
| monument | full | monster: guardian 2–4; axolotls and underground water: none |
| pillager outpost | full | monster: pillager ×1 |
| ancient city | full | all eight: none |
| trial chambers | piece | all eight: none |
| fortress | piece | (Nether) |

### 8.4 The types' rules (SpawnPlacements.txt 77–100 and its static table from 102)

Peaceful refuses a type not allowed in it first. The table names each type's placement,
heightmap and predicate by its invokedynamic index; which method each index is, is
**Inferred** (a `javap -v` names them: section 14).

| Rule | Types | Draws (level random), in order |
|---|---|---|
| `Monster.checkMonsterSpawnRules` (134–154): light unless trial spawner, then `below.isValidSpawn` | zombie, zombie villager, zombie horse, skeleton, bogged, creeper, spider, cave spider, enderman, witch, the illagers, vex (no restrictions) | `isDarkEnoughToSpawn` (81–132): raw sky light > `nextInt(32)` → no (**always drawn**); block light > the dimension's limit (overworld 0) → no; darkened raw light (10-block reach in thunder) ≤ `monster_spawn_light_level` sample (overworld uniform 0–7: `nextInt(8)`) |
| `checkSurfaceMonstersSpawnRules` (166–185) | husk, parched, camel husk | the above, then `canSeeSky` |
| stray (Stray.txt 11–42, n7k) | stray | the above, then sky above any powder snow |
| drowned (Drowned.txt 296–390, n7k) | drowned (in water) | not water below → no, **no draws**; light draws; in `#more_frequent_drowned_spawns` (rivers) `nextInt(15) == 0`, else `nextInt(40) == 0 && y < sea − 5` — drawn whatever the light said |
| guardian (Guardian.txt 796–829) | guardian, elder (in water) | `nextInt(20)` first, always; then not seeing the sky from below the water, water here and below |
| slime (not dumped, **Inferred**) | slime | swamps 50 < y < 70: `nextFloat() < 0.5`, `nextFloat() < moon`, light ≤ `nextInt(8)`; slime chunks (`seedSlimeChunk(cx, cz, seed, 987234911)`) `nextInt(10) == 0`, y < 40 |
| `PatrollingMonster` (151–167) | pillager | block light > 8 → no; else any light |
| `Animal.checkAnimalSpawnRules` (Animal.txt 174–212) | the farm animals | raw undarkened light > 8 and the block below in `animals_spawnable_on` |

### 8.5 The overworld's monster lists (`worldgen/biome/*.json`, weight × count)

- **Default** (28 biomes: forests, taigas, peaks, badlands, beaches, meadows, …): spider
  100×4, zombie 95×4, zombie villager 5×1, skeleton 100×4, creeper 100×4, **slime 100×4**,
  **enderman 10×1–4**, witch 5×1 (515).
- Plains and the savannas: zombie 90 and **zombie horse 5×1**. Snowy plains and ice spikes:
  skeleton 20, stray 80×4. Desert: zombie 19, zombie villager 1, skeleton 50, husk 80×4,
  parched 50×4. Swamps: skeleton 70, a second slime 1×1, bogged 30×4. Oceans: drowned 5×1;
  river drowned 100×1, frozen river 1×1, dripstone caves 95×4. Jungles: ocelot in the
  monster list. Old growth pine taiga: zombie 100, zombie villager 25. Sulfur caves (new):
  sulfur cube 100×2–4 and others. **None**: deep dark, mushroom fields, the void.
- Order within a list matters to the weighted pick.

### 8.6 Despawning (built)

`Mob.checkDespawn` (Mob.txt 1605–1703) is Clustine's `check_despawn` (`mobs/body.rs`
499–556), all but peaceful's branch: discarded beyond the category's despawn distance where
`removeWhenFarAway`; one in 800 (own) beyond 32 once idle more than 600; idle reset within
32. `Monster.updateNoActionTime` adds 2 a tick in light worth more than 0.5 (Monster.txt
27–42; built). Who stays: animals, golems, villagers, traders, the allay, axolotls, fish,
the nautili, cats, ocelots, a chicken jockey, a zombie horse, the camel husk, a converting
zombie villager or one with experience, a raider in a raid, a patrolling one within 128.
Finding 28: its nearest player must be the world's.

### 8.7 The custom spawners (after random ticks, overworld only)

Order **Inferred** (MinecraftServer not dumped): phantoms, patrols, cats, the village
siege, the wandering trader. **Cats** (CatSpawner.txt 13–118): every 1200 ticks, a random
player (level draw), offsets `nextInt(24)` and `nextBoolean` per axis (±8 to 31), then a
village (more than 4 occupied homes within 48 and fewer than 5 cats) or a hut — needs
points of interest (N10). **The wandering trader** (WanderingTraderSpawner.txt 25–200): its
own `RandomSource.create()`, a chance rising 25 a day to 75, a point of interest within 48 —
N10's. **Phantoms, patrols, the siege**: not dumped; phantoms need insomnia (the player's
time since rest: survival's statistic), patrols day 5 and `nextInt(5) == 0` every 12000 to
13200 ticks.

---------------------------------------------------------------------------------------

## 9. The draws, in the game's order

| Moment | Stream | Draws |
|---|---|---|
| a target goal asked (each selector tick, each slot) | own | `nextInt(5)` (interval 10); the witch's healer `nextBoolean()` first; the spider's in light: none |
| `MeleeAttackGoal` path | own | `4 + nextInt(7)` at a recalculation; `nextFloat() < 0.05` at a still target |
| spider attack in light | own | `nextInt(100)` per selector tick |
| leap | own | `nextInt(reducedTickDelay(5))` |
| zombie hit while burning | own | `nextFloat()` (eff · 0.3) |
| skeleton strafing | own | two `nextFloat()` every 20 strafing ticks |
| skeleton shot | arrow: UUID, 2 (damage triangle), 6 (shot); own: 1 (sound) | |
| pillager | own | `nextInt(21)` per path (**Inferred** UniformInt), `nextInt(20)` per charge; arrow UUID + 6 |
| witch throw | own: [`nextFloat` weakness], [`nextFloat` sound unless silent]; potion: UUID + 6 | |
| drowned throw | trident UUID + 6; own 1 (sound) | |
| evoker summon | own `nextInt(8)` (after the parent passes); three × (`nextInt(5)`, `nextInt(5)`, the vex's level draws, `nextInt(90)`) | |
| evoker wololo | own `nextInt(size)` | |
| vex | own `nextInt(4)` (charge), `nextInt(4)` (move), up to 3 × three `nextInt` (move) | |
| arrow in flight | own: 3 when it falls from a broken block; 1 per hit sound; `nextInt(j/2 + 2)` crit; 1 at a deflection | |
| egg hit | own `nextInt(8)`, then `nextInt(32)` | |
| player throws | level: the sound's `nextFloat()` (before the spawn); projectile UUID + 6 | |
| player bow | arrow UUID + 6; level 1 (sound, after) | |
| spawning, per tick | level | the shuffle (`size − 1`); per chunk and category: 3 position; per group 1 pack size; per attempt 4 steps, [1 type, [count]] , the rule's, 1 yaw, `finalizeSpawn`'s |
| despawn | own | `nextInt(800)` when idle > 600 and not kept |

**Inferred** throughout where a class is not dumped. In Clustine "level" is the keyed
stream of what draws (A28 section 4): a chunk's for spawning, the region's aside for a
player's throw, an entity's for its own.

---------------------------------------------------------------------------------------

## 10. What clients are sent

| What | Data or packet |
|---|---|
| a melee hit | `ClientboundSwingAnimationPacket` (the swing), the hurt (built), the attack sound |
| zombie arms | Mob's flags (15) bit 4, aggressive |
| a skeleton, drowned or pillager using its weapon | living flags (8) bit 1 and 2; the equipment update at `startUsingItem` |
| pillager charging | 17 boolean; the loaded look needs `charged_projectiles` in the stack (a component) |
| evoker | 17 spell byte; its prepare and cast sounds |
| vex | 16 flags (charging); `vex_charge` |
| guardian | 17 the target's **network id** (mapped per client); entity event 21 |
| arrows | data 8, 9, 10 (11 a tipped colour); always position and rotation; `TakeItemEntity` at pick-up; the add packet's data the owner |
| tridents | 8–10, 11 loyalty, 12 foil |
| a living entity with arrows in it | 12 arrow count |
| thrown items | 8 the stack (built) |
| splash | level event 2002 or 2007 with the colour |
| egg, snowball | entity event 3 |
| fangs | entity event 4 |
| glowing | shared flags bit 6 |
| a player drawing a bow | living flags of that player, to the others |
| mob effects on a player | `UpdateMobEffect`, `RemoveMobEffect` (only if section 15 question 7 says so) |

---------------------------------------------------------------------------------------

## 11. What crosses a line

- **A target beyond the line** is a key, resolved in the region's land, then its copies, then
  its players (`region/mobs.rs` 66–84); a target that has gone beyond the copies or is in
  flight between regions is lost when `canContinueToUse` fails, as at the edge of a game's
  loaded area. A28 section 7 says regions that follow players **merge** when an entity
  "targets or is targeted across" the line; **that is not built** (only crossings and
  interactions note work at a line, `arrivals.rs` 92; N7n). A horde chasing villagers over
  a line that follows players will either merge or hesitate.
- **A hit** on what only a copy shows is an entity effect to its holder (`EntityUse::MobHit`,
  `brains.rs` 96–163), as the golem's is today. N9's hits add what the holder must apply only
  if the hurt took: the zombie's fire (the draw is the attacker's, made before it knows —
  the game draws only after a hit took: **the order differs at a line**, a question), the
  husk's hunger, effects, Punch, the arrow count.
- **An arrow** crosses as an entity; its hit across a line is an effect; its own fate depends
  on the hurt (finding 16).
- **A splash** reaches every living thing within 4: those in copies need an effect each
  (`EntityUse::Splashed`, with the proximity), the region's own directly.
- **Fangs** are made up to 20 blocks from the evoker and hurt what is in their box: made in
  their own chunk's region (`send_made` where the land is another's), hurting there. A line
  of 16 fangs across a line is ordinary.
- **Vexes** appear within 2 blocks of the evoker and fly without physics: they cross as any
  entity.
- **`alertOthers`** reaches mobs in copies: a "set your target" effect, or only the region's
  own (a question).
- **A rider and its vehicle** are one unit in one chunk and one region (riding.rs 10–17):
  steering never crosses a line.
- **The guardian's beam** names an entity the client must know: one across a line is in the
  edge's view if the client sees it; the region reports its key.
- **Spawning**: a pack's steps (up to ±5 a step, 4 a group) leave the chunk; the game refuses a
  position only in a chunk that cannot tick (`canSpawnEntitiesInChunk`). A position in
  another region's land is that region's: a mob made there is sent (`send_made`), or the
  attempt refused (a question). **The counts** — the global count, the spawnable chunks,
  each player's local count — are the world's (A28 section 8: the store publishes the
  mobs by category of each chunk that changed and the totals, a few ticks late; not
  built). **The nearest player** for the 24 blocks and for despawning beyond the copies is
  the world's players' table, a second late (finding 28).
- **The order of spawning** across regions: the game shuffles all the level's spawning
  chunks with one random; Clustine's regions each spawn their own chunks with each chunk's
  stream, in parallel; with the caps a few ticks late, the order matters only to who takes
  the last places under a cap.
- **Inhabited time** is a chunk's, raised by the region that ticks it, and saved with it, if
  kept.

---------------------------------------------------------------------------------------

## 12. What a player meets within minutes

- **Night falls and monsters come** (natural spawning): within a minute of dark, zombies,
  skeletons, creepers and spiders around them in the open and in caves; in a desert husks
  and parched, in snowy plains strays, in swamps bogged and slimes, drowned in rivers,
  witches in swamp huts, guardians at monuments; they burn at dawn (built) and wander off
  (despawning, built). **No slimes and no endermen** without those types (finding 26).
- **A village at night**: zombies break nothing at easy but kill villagers (who stay dead at
  easy), fight the iron golem, chase turtles; the golem fights back and wins; a snow
  golem's snowballs draw creepers to it, which explode.
- **Skeletons shoot golems and wolves**; an arrow that hits the player bounces off; arrows
  stick in the ground and in mobs (the arrow count), and can be picked up by the player who
  shot them.
- **Their own bow, crossbow, trident, eggs (chicks), splash potions, experience bottles and
  ender pearls** — the first things a creative player tries from the combat tab — and a
  dispenser that shoots arrows.
- **A spider jockey** that moves as its skeleton wants, a chicken jockey that chases.
- **An evoker** by blue sheep turning them red; summoning vexes at a golem.
- **What they will not see, named or a question**: mobs attacking them (never, in creative),
  a potion's effect on themselves, tipped arrows and potion colours without components,
  phantoms (survival's insomnia), patrols and raids, the skeleton trap (weather).

---------------------------------------------------------------------------------------

## 13. What Clustine has to reuse, and what is missing

- **Goals** (`mobs/ai/goals.rs`): `TargetRules` (1288–1399), `HurtByTargetGoal` (1400–1468, no
  alerting), `NearestAttackableTargetGoal` (1470–1545, with a selector of `fn(&Looked)`; one
  that reads the world exists only as `tamable::nearest_target`, `tamable.rs` 654–678),
  `MeleeAttackGoal` (1547–1715), `LeapAtTargetGoal` (1716–1757), `MoveTowardsTargetGoal`
  (`iron_golem.rs` 170–230), the creeper's `SwellGoal` (`creeper.rs` 115–151), the
  `RangedAttackGoal` twice (`snow_golem.rs`, `llama.rs`), strafing (`controls.rs` 35, 93),
  `Mob.lookAt` three times (`cat.rs` 403, `nautilus.rs` 611, `panda.rs` 805). Wrappers to copy:
  the polar bear's, the wolf's, the ocelot's.
- **Placeholders**: `AttackOfN9` and `TargetOfN9` (`monster_ai.rs` 42–105) in the skeletons,
  creeper and spider; `tamable::Idle` in the zombie family, drowned, illagers, witch and
  guardians (finding 4). `MobType::do_hurt_target` exists (`types.rs` 295–300; the iron golem
  overrides it).
- **Sensing** (`sensing.rs`): the creative locks; visibility with crouching only (146–155):
  invisibility and the worn head are missing; `PlayerFace.worn` exists (`api.rs` 217–218)
  but is dropped.
- **Hurt** (`body/hurt.rs` 331–340): `last_hurt_by` and `last_hurt_by_player` kept,
  persisted (`body.rs` 207–215). No path to hurt a player (`mobs/tick.rs` 100–104), right
  while everyone is creative.
- **Projectiles** (`region/thrown.rs`, `physics/thrown.rs`): `make_thrown` (the key, the
  stream, the UUID, `shoot` or `shoot_from_rotation`, `send_made` across a line), the hit
  test (`targets`, `can_hit`, `clip_blocks`, `margin`), `write_thrown` with `relocate`,
  `EntityUse::Thrown` (tag 10), `Local::Shoot` (`mobs/tick.rs` 216) — **hard-wired to the
  snowball** (`snowball_kind()`, damage 0, entity event 3, the region's own players only,
  the neutral source). `golem_aim` is the golem's and witch's aim; the skeleton's needs
  `getY(1/3)` from the projectile's own y. `EntityData::Thrown` fits every thrown item; arrows
  and tridents need a variant of their own. The edge sends the owner's wire id and the stack
  at 8 for `Thrown` (`fanout.rs` 6356–6369, `tracking.rs` 904–914).
- **Using items**: none in general; the wandering trader's own (`wandering_trader.rs` 42–46)
  and the witch's (`illagers.rs` 64–68).
- **Effects** (`body/effects.rs`, 429 lines): 40 ids, hidden layers, `add`/`update`, the three
  tags of `canBeAffected`, the speed modifier, instant health and harming without a splash's
  factor, the countdown. Missing: the periodic ticks, slowness, weakness, the types'
  refusals, glowing, the splash, players' effects, particles.
- **Riding**: `tick_passengers_of` (`riding.rs` 798–855), the unit in one chunk;
  `update_control_flags` correct but fed nothing (`ai.rs` 739–766); `RandomStrollGoal.can_use`
  leaves out `hasControllingPassenger` (`goals.rs` 351–352); navigation and controls read
  `ai.tick.mob` only (`navigation.rs` 174–205, 382–402; `controls.rs` 22–60).
- **Spawning** (`generation/spawning.rs`, N7l): `pick` (172, the weighted list exactly;
  private), `is_spawn_position_ok` (304), `is_valid_empty_spawn_block` (327), the border, the
  spawn box (337), `contains_any_liquid` (363), `biome_in_tag` (428), `magic_value` (477);
  `clustine_data::spawning` (placements, `is_valid_spawn`, `is_block_dangerous`),
  `BIOME_SPAWNS` with every category and the costs, the tags. Not reusable as they are:
  `top_non_colliding_pos` and `adjust_spawn_position` (generation's); `no_collision` (no
  entities); `check_spawn_rules` (creatures only, no draws); `raw_brightness` (undarkened:
  right for animals, not monsters; `region/light.rs` 247 has the darkened one);
  `walk_target_value` (a creature's); `made_mob` (generation's making: day 0, full moon,
  difficulty (0, 0)). The runtime making is `Region::spawn_mob_with` (`region/spawn_eggs.rs`
  158–) with `SpawnReason::Natural` (`mobs/types.rs` 1163). `SpawnView` is generation's: the
  region needs one over its chunks, light and biomes; it knows each cell's noise biome, not
  the zoomed one (named, `region/animals.rs` 7–10).
- **Despawning**: built (`body.rs` 499–556, `body/ladder.rs`), reading the region's players
  and its copies' faces, not the world's table.
- **Not there**: inhabited time, weather and thunder, structure starts at runtime (for the
  overrides), the world spawn's 24 blocks, the game rules, the custom spawners, the published
  counts, slime and enderman.

---------------------------------------------------------------------------------------

## 14. Not dumped (the lead dumps them)

**Projectiles and their items** (`javap -p -c -constants`): `projectile.arrow.AbstractArrow`
(above all `tick`, `stepMoveAndHit`, `findHitEntity`, `canHitEntity`, `onHitEntity`,
`onHitBlock`, `doKnockback`, `shouldFall`, `startFalling`, `tickDespawn`, `playerTouch`,
`tryPickup`, `setOwner`, `setBaseDamageFromMob`, constructors, `getWaterInertia`, synched
data, save), `arrow.Arrow`, `arrow.SpectralArrow`, `arrow.ThrownTrident`;
`throwableitemprojectile.ThrownEgg`, `ThrownEnderpearl`, `ThrownExperienceBottle`,
`AbstractThrownPotion`, `ThrownSplashPotion`, `ThrownLingeringPotion` (package to confirm);
`AreaEffectCloud`, `projectile.FireworkRocketEntity` (if in scope), `projectile.EvokerFangs`;
`BowItem`, `CrossbowItem` with `CrossbowItem$ChargingSounds`, `ProjectileWeaponItem`,
`TridentItem`, `EggItem`, `SnowballItem` (to confirm the inferred throw), `ThrowablePotionItem`,
`SplashPotionItem`, `LingeringPotionItem`, `ArrowItem`, `TippedArrowItem`,
`SpectralArrowItem`, `ExperienceBottleItem`, `EnderpearlItem`, `FireworkRocketItem`;
`ProjectileItem$DispenseConfig$Builder`; `monster.CrossbowAttackMob`;
`alchemy.PotionContents`, `component.ChargedProjectiles`.

**Effects**: `PoisonMobEffect`, `WitherMobEffect`, `RegenerationMobEffect`,
`HealOrHarmMobEffect`, `InstantenousMobEffect`, `HungerMobEffect`,
`MobEffect$AttributeTemplate`.

**Spawning**: `LocalMobCapCalculator` (with `MobCounts`), `PotentialCalculator`,
`DistanceManager` (`getNaturalSpawnChunkCount`, `getSpawnCandidateChunks`,
`hasPlayersNearby`), `FixedPlayerDistanceChunkTracker`, `ChunkTracker`; `MinecraftServer`
(the custom spawners' order, `setSpawnSettings`'s caller); `PhantomSpawner`,
`PatrolSpawner`, `VillageSiege`; `monster.Slime`, `monster.EnderMan` (and its goals),
`Silverfish`, `Phantom`, the sulfur cube; `WorldgenRandom.seedSlimeChunk`;
`ChunkAccess.getHeight`, `CollisionGetter.noCollision` and `isUnobstructed`,
`StructureManager.structureHasPieceAt`; **`SpawnPlacements` with `javap -v`**, to name each
predicate behind its invokedynamic index; how the moon's brightness is read in 26.3.

**Other**: `EntitySelector` of 26.3 (`NO_CREATIVE_OR_SPECTATOR`, `NO_SPECTATORS`),
`Avatar.canBeSeenAsEnemy`, `GameType.updatePlayerAbilities` (to confirm creative is
invulnerable), `EnchantmentHelper.enchantItemFromProvider` (does it draw at a special
multiplier of 0?), `Level.destroyBlockProgress`, `LightningBolt`.

Dumped and not read for this brief: `MoveThroughVillageGoal`, `RaiderOpenDoorGoal`,
`OpenDoorGoal`, `DoorInteractGoal`'s tick in full, `LivingEntity$SwingState`,
`SpearUseGoal`, `UniformInt`, `TimeUtil`.

---------------------------------------------------------------------------------------

## 15. Open questions for the plan, each with options and a recommendation

1. **Which projectiles are in N9?** (a) arrows (plain, spectral, mob variants), tridents,
   eggs, splash potions, experience bottles, the llama's spit, dispensers of these, the bow
   and the trident in a player's hand; (b) as (a) with the crossbow, ender pearls and
   fireworks; (c) as (b) with lingering clouds. **Recommended: (b)**: a creative player tries
   the bow, crossbow, pearls and rockets from the combat tab within minutes; lingering
   clouds (`AreaEffectCloud`, an entity that acts across a line every 5 ticks) named, unless
   dumped and cheap. An ender pearl teleports the owner where their region holds them, and
   across a line by the player's hand-over (a design of its own, not delegated).
2. **Item components.** (a) a small typed set now — `potion_contents` (potion id, colour,
   custom effects), `charged_projectiles`, `fireworks` flight, enchantments' ids and levels —
   fixed as a shared type first, with the store's format raised; (b) none, the losses named
   (tipped arrows plain, potions without effects, a crossbow drawn unloaded, rockets of no
   flight). **Recommended: (a)**, as its own first step: tipped arrows, potions and the
   crossbow's look are what a player sees at once, and the witch's potion and the patrol
   leader's banner (P 10) come with it.
3. **Using an item.** (a) one `using { hand, item, started }` on the living body, ticked
   before AI as the game's, folding in the trader's and the witch's, with the living flags,
   kept in the record (a player's moved with them at a hand-over); (b) per type. **Recommended:
   (a)**; the player's side reads a new `PlayerInput::ReleaseUse` and is the lead's (the
   hand-over).
4. **An arrow's hit across a line, whose outcome needs the hurt.** (a) the sender judges on its
   copy (alive, invulnerable ticks) and sends the outcome; (b) the arrow is gone at a hit across
   a line, the holder hurting as it can (the bounce named); (c) the arrow itself crosses and
   hits in the holder's region (a hand-over of the arrow as an arrival in the same tick
   while coupled). **Recommended: (b)**, with regions that follow players merging for it
   (A28 section 7); the bounce shows only at a line under timing.
5. **Steering a vehicle.** (a) the rider's tick runs with its controlled vehicle's
   navigation and move control (a second `Ai` over the vehicle's record, written back
   within the region's tick); (b) orders to the vehicle's next tick; (c) a goal on the
   vehicle that follows the rider's target. **Recommended: (a)**, the lead's: only it keeps
   paths read back at once and the draws; with the real control flags, the stroll's
   `hasControllingPassenger`, the body-yaw copy of `rideTick` and the vehicle's box in the
   attack reach.
6. **Mob effects.** (a) slowness, poison, weakness, regeneration, instant health and harming
   with a splash's factor, glowing, hunger as nothing on mobs, and the types' refusals;
   (b) all forty. **Recommended: (a)**, after dumping the subclasses (their intervals are
   what the tests check).
7. **Effects on players.** (a) none, named (only a splash beside a fight reaches a creative
   player); (b) a player's effect store with the effect packets. **Recommended: (a)** in
   N9; it becomes (b) with survival. A player's own splash potion thrown at their feet is
   the case they would try: name it plainly.
8. **The difficulty.** (a) easy only, every rule reading `Difficulty::WORLD` and its id, the
   effective difficulty computed from the clock and the moon with inhabited time 0 (named);
   (b) as (a) with each chunk's inhabited time kept, raised by the region that ticks it and
   saved with the chunk; (c) normal or hard allowed. **Recommended: (a)**: at easy inhabited
   time changes `(int) eff` only after about 33 hours of play in one chunk; normal and hard
   wait for the enchantment providers (P 10). The `(0, 0)` that eggs and generation pass
   becomes `(eff, 0)`.
9. **Replacing the placeholders.** All target goals at once, with the stream changes and the
   tests that pinned them changed in the same commit; or type by type. **Recommended: all
   at once**, the lead's first commit, so that the draw order is right even where nothing is
   targeted.
10. **`alertOthers` across a line.** (a) a "take this target" entity effect to mobs in
    copies; (b) only the region's own, named. **Recommended: (a)**, cheap beside
    `MobHit`, and a horde that half answers at a line is a seam.
11. **Which mobs.** The vex is needed with the evoker; slime and enderman for spawning
    (finding 26; the slime's chunks need the world seed, its swamp rule the moon); the
    ravager and the illusioner left out. (a) slime and enderman in N9 with their spawn rules
    and goals; (b) left out, and their weight skipped (a named difference in counts).
    **Recommended: (a)**: without them a fifth of all overworld monster positions fail and
    slimes in swamps are part of the world a player knows; the enderman's block carrying
    and teleport are its own part, dumped first.
12. **Natural spawning across regions.** (a) the store publishes A28 section 8's counts (each
    chunk's mobs by category, the world's totals and spawnable chunks) and the world's
    players (built, once a second); each region spawns in the chunks it holds that are
    spawning chunks, with each chunk's stream, reading the global cap and each player's
    local count from the published values plus its own changes since; (b) a region counts
    only what it holds and copies, each region taking the caps' share of its players.
    **Recommended: (a)**, A28's, the lateness named. The questions inside it: how late is
    allowed (one second, as the players' table?); what a region does before the first
    publication (spawn nothing, rather than all); whether the local cap counts the
    region's own changes at once (yes: the game's `afterSpawn` raises it within the tick).
13. **A pack that steps into another region's land.** (a) refuse the attempt (as the game
    refuses a chunk that cannot tick); (b) make the mob and send it (`send_made`). **Recommended:
    (a)**: the chunk is ticked by the other region, which spawns its own; the game's
    `canSpawnEntitiesInChunk` is the rule to read, and refusing keeps the counts local and
    exact. Named: packs near a line are slightly fewer.
14. **The nearest player for spawning and despawning.** Use the world's players' table
    (published once a second) where the region's own and its copies' faces do not reach.
    **Recommended**: yes, for despawning too; until a table has come, a region spawns
    nothing and despawns nothing, as n7-plan 9.2 has "every player distance near".
15. **Structures at runtime** (the swamp hut's witches, the monument's guardians, the
    outpost's pillagers, ancient cities and trial chambers silent). (a) keep each chunk's
    structure starts and pieces' boxes (generation has them, S-plan) for the spawner;
    (b) biome lists only, named. **Recommended: (a)**: witches at huts and guardians at
    monuments are what a player finds, and an ancient city full of zombies would be a seam.
16. **The custom spawners.** Cats and the wandering trader need points of interest (N10);
    phantoms need a survival statistic; patrols need raids. **Recommended**: none in N9,
    named.
17. **The merge for targets across a line** (A28 section 7, not built). (a) build it in N9:
    an entity that targets or is targeted across the line counts as work there;
    (b) leave targets to resolve through copies. **Recommended: (a)** where regions follow
    players, tested with a horde chasing villagers over a line both ways; pinned lines keep
    effects.
18. **The zombie's fire on a hit at a line** draws before the attacker knows whether its hit
    took (the hurt is the holder's). (a) draw at the attacker as if it took, the holder
    lighting the target only if it did; (b) send the probability and let the holder draw.
    **Recommended: (a)**, named: a draw made on a refused hit across a line.
19. **Comparison with the official server.** Exact where nothing draws: a summoned arrow's
    flight with a fixed motion (`/summon arrow ~ ~ ~ {Motion:[…]}`, `/tick step`), an
    egg's or a potion's flight, the guardian's beam timing, fangs' positions for a fixed
    evoker and target, a skeleton's rhythm by its `attackTime`; by distributions: spawning
    per hour in a dark flat world with a fixed light, by type and height, against the
    official server with `spawn_monsters`; the wire's packets for each projectile.
    **Recommended**: these, with the dumps first.
20. **Order of work.** The lead first: the dumps; components and `ItemStack`; the
    projectile entity variants; the use state and `ReleaseUse`; `EntityUse` additions
    (alert, splash, a projectile's hit with its extras); the real target goals; the
    controlled vehicle; the published counts. Then subagents with crates of their own:
    melee and the zombie family; skeletons, pillager, witch, drowned and guardians;
    the evoker and the vex; arrows, tridents and pick-up; thrown items and dispensers;
    effects; spawning's rules and slime and enderman; the edge's data. Tests from the
    specification by someone who did not write the code, as before.
