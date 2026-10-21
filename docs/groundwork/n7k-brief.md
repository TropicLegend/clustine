# N7k porting brief: golems, villagers and monsters' bodies (26.3, protocol 777)

Read from `server-26.3.jar` with `javap -p -c -constants` (names are Mojang's), one file per
class, in `/home/claude/reference/n7-javap/` (with `n5-javap/`, `n6-javap/` and `n7d-javap/`
beside it), and from the datagen's output in `target/datagen/26.3/generated/` (tags, loot
tables, timelines, dimension types, enchantment providers) and Clustine's own extraction
(`crates/clustine-data/src/generated/`: `attributes.rs`, `entity_types.rs`, `items.rs`,
`loot_tables.rs`). Every rule below is the bytecode's or the data's unless marked
**Inferred** (reasoning, to test) or **Client** (client code, not in the server jar). Line
ranges are of the `javap` output of the class named ("IronGolem.txt 362–442" is
`net.minecraft.world.entity.animal.golem.IronGolem`'s); a file of another dump says so.
Cited as B: `n7-brief.md` (B 8.15 is its section 8.15), P: `n7-plan.md`, L: `n7l-plan.md`.

The manner is B's: what the step should know first (section 0), then type by type, then the
draws (section 5), what clients are sent (section 6), the line between regions (section 7),
what Clustine has (section 8), what was not dumped (section 9) and the open questions
(section 10). The conventions of B's preamble hold: `double` is `f64`, `float` is `f32`, a
`float` constant in `double` arithmetic is widened after the float arithmetic it takes part
in, no fused multiply-add, `Mth.floor(double)` is `x.floor() as i32`.

Where it sits: P 11.5 names N7k "golems, villagers, monsters' bodies", in the second half,
after N7j; P 11.2 (scope) has "the three golems; villagers with the core and idle
activities and their schedule's rest; monsters and other mobs from eggs and structures as
bodies that float, stroll and look"; P 11.6 names "villagers do no work, sleep in no bed
and trade nothing until N10; monsters do not attack or burn until N9". L 9 item 12 moves
**burning in daylight** into N7k ("a zombie village's zombie villagers would stroll
unburnt in daylight … a seam of ordinary play"), and L 5.4 builds the structures' entities
(D3) on N7k's bodies, after N7k.

---------------------------------------------------------------------------------------

## 0. What the step should know before planning (findings)

**Burning**

1. **Burning in daylight is `Mob`'s, by a tag, not the zombie's or the skeleton's**
   (Mob.txt 884–893, 985–1088). `Mob.aiStep` calls `burnUndead()` for every type in
   `#burn_in_daylight`: skeleton, stray, wither skeleton, bogged, zombie, **zombie horse**,
   zombie villager, drowned, **zombie nautilus**, phantom. The husk and the parched are not
   in it. `Zombie.isSunSensitive` (Zombie.txt 444) and `Husk.isSunSensitive` are called by
   no dumped class. Two types Clustine has built already, the zombie horse and the zombie
   nautilus, burn in the game and do not in Clustine: N7k's burning mends them too (3.3).
2. **When it burns is data**: the environment attribute `gameplay/monsters_burn`, false by
   default (n7d EnvironmentAttributes.txt 556–567), is set by the overworld's `day` timeline
   (`timeline/day.json` 90–102: false from tick 12542 of the day, true from 23460, modifier
   `or`). Then the light at the eye (`getLightLevelDependentMagicValue` above 0.5, which in
   the overworld is a raw brightness of 7 or more), a draw of the mob's **own** random
   (`nextFloat() × 30 < (b − 0.4) × 2`, made whenever the time and light pass, before the
   wet and sky checks), not wet (water, rain, powder snow now or last tick), and `canSeeSky`
   at the eye. At full light the chance is 0.04 a tick (3.3).
3. **A helmet protects and wears**: with anything on the head (`sunProtectionSlot`, `HEAD`),
   no fire; a damageable helmet loses `random.nextInt(2)` of durability (a second own draw);
   a pumpkin protects and draws nothing. Without, `igniteForSeconds(8)`: 160 ticks scaled by
   the `burning_time` attribute and rounded up, never lowering a longer fire (Entity.txt n6
   1271–1290, LivingEntity.txt 11078–11098). Clustine's fire ticks, the flag and fire's
   damage exist (`clustine-sim/src/living.rs` 200–456).

**Difficulty**

4. **Most of the monsters' spawn draws are multiplied by the special multiplier**, which is 0
   on easy, on normal for the first three days, and while Clustine passes `(0, 0)`
   (`Spawning.difficulty`, every caller): loot pick-up, door breaking, armour, enchantment,
   the patrol leader's chance for some, the zombie's follow-range bonus and leader. The draws
   are still made. What does not depend on it: a zombie's baby (0.05), its chicken (0.05 and
   0.05), its weapon (0.01, 0.05 on hard), the patrol leader's 0.06, the pillager's one in 300
   Piercing crossbow, the Halloween pumpkin. So at special 0 no monster wears armour or holds
   an enchanted item, and nothing needs enchanting until difficulty exists (section 10 asks
   which difficulty Clustine runs at).
5. **The zombie's `handleAttributes` draws from its own random** (Zombie.txt 1202–1300), up to
   six draws after the level's; its knockback-resistance bonus (`random_spawn_bonus`, 0 to
   0.05) and reinforcement chance (0 to 0.1) are made at every spawn but a conversion's, and
   the follow-range bonus and leader only above special 0.67 and 0 respectively.

**Construction draws**

6. **A zombie villager draws its profession in its constructor** (ZombieVillager.txt 44–69:
   `defineSynchedData` calls `initializeZombieVillagerData(this.random)`, a
   `VILLAGER_PROFESSION.getRandom`), from its own stream, before anything else; a guardian
   draws `nextFloat()` for its tail in its constructor (Guardian.txt 27–56). These are made
   for every construction, also an egg's child that is refused (B 1.11.4).

**Golems**

7. **A built golem is not finalised**: `CarvedPumpkinBlock.trySpawnGolem` (75–170) creates it
   with `TRIGGERED` and never calls `finalizeSpawn`: no draw, no follow-range triangle, no
   left hand; an iron golem is marked player-created. It is placed at the pattern's bottom
   cell `(x + 0.5, y + 0.05, z + 0.5)`, yaw and pitch 0, after the pattern's blocks are set
   to air with level event 2001 each, and before their neighbours are updated (215–341).
   Clustine's copper golem path (`region/copper_golems.rs`) is the same shape, hand-written
   because `BlockPattern` is not dumped; the iron and snow golems' shapes are given here
   (1.5), the search over orientations is not (section 9).
8. **The iron golem's attack is its own** (`doHurtTarget`, IronGolem.txt 362–442): damage
   `15 / 2 + nextInt(15)`, 7.5 to 21.5, and on a hit the target's motion gains
   `0.4 × max(0, 1 − its knockback resistance)` upwards, besides the hurt's own knockback.
   Clustine's `melee_hit` (`mobs/ai/goals.rs` 1569) hurts with the attack damage and no
   knockback: a hook for a type's `doHurtTarget` is needed.
9. **With monsters' bodies in the world the golems find targets in ordinary play**: an iron
   golem targets any `Enemy` but a creeper within its follow range 16 (every fifth tick's
   search), walks at it and hits it; a snow golem targets any `Enemy` within 16 and throws
   snowballs, which Clustine cannot make (no projectiles). A player who builds a snow golem
   by a zombie sees this within minutes (1.6, section 10).
10. **The iron golem's flag is bit 0** (mask 1) of data 16 (IronGolem.txt 615–662); B 8.15's
    "bit 1" means the first bit. The cracks are the client's from the health (**Client**);
    the server sends only health and plays `iron_golem_damage` when the crack level changes
    (`Crackiness` not dumped).
11. **A moving iron golem draws every tick**: `canSpawnSprintParticle` (IronGolem.txt
    244–261) is called by `Entity.baseTick` on the server too (n6 Entity.txt 963–1000) and
    draws `nextInt(5)` while the golem moves; one tick in five, `spawnSprintParticle`
    (n6 Entity.txt 4624–) draws two `nextDouble()` more where the block below is visible. P's
    rule "particle draws made and discarded" covers them.
12. **A snow golem places snow while it melts**: `SnowGolem.aiStep` (127–247) hurts it by
    `onFire` 1 where `gameplay/snow_golem_melts` holds, then, with `mob_griefing`, places snow
    at the four corners ±0.25 of its feet wherever the block is air and a snow layer can
    survive (`SnowLayerBlock.canSurvive` reads only the block below). It is hurt by water
    (`isSensitiveToWater`, `drown` 1 a tick in water or rain, LivingEntity.txt 8590–8600).

**Villagers**

13. **An unemployed villager refuses a player as the game's does**: `Villager.mobInteract`
    (279–337) with no offers plays `villager_no` and sets the unhappy counter to 40
    (`setUnhappy`, 338–351); unemployed and nitwit professions have no trade sets
    (`VillagerProfession.getTrades` is null for them, **Inferred** from their registration,
    VillagerProfession.txt 84–124). Every villager an egg or a village makes in N7 is
    unemployed, a nitwit or a baby (B 8.0.20), so "refused" is exact without trading. A
    wandering trader always has offers (WanderingTrader.txt 282–303), and the game opens its
    screen (section 10).
14. **A villager without a brain sinks**: floating is the brain's `Swim(0.8f)` in `CORE`
    (B 7.14.1); the body alone does not rise in water. "Floats" needs at least the core
    package. Every tick a villager draws `random.nextInt(100)` unless `NoAI`
    (`customServerAiStep`, Villager.txt 190–261), raid or not, and picks up items in
    `#villager_picks_up` or with the `villager_food` component into its eight slots
    (`Mob.aiStep`'s looting, Mob.txt 900–977; `Villager.wantsToPickUp`, 1567–1598).
15. **`VillagerData` is serializer 18** (EntityDataSerializers.txt, registration order):
    type and profession as registry ids, then the level, all var-ints; the finalised flag is
    synched data of its own (a boolean beside it, Villager.txt 750–767). Clustine's edge has
    no `VILLAGER_DATA` writer.
16. **The villager's type is by biome at its feet unless finalised**
    (`VillagerDataHolder.finalizeVillagerType`): seven types, 33 biomes named, plains for
    the rest (VillagerType.txt 79–242); a saved `VillagerData` marks it finalised when read
    (Villager.txt 832–920), which is how a template's villager keeps its type (B 8.12).

**Monsters**

17. **The witch draws four times a tick when it is not drinking** (Witch.txt 181–406), for
    water breathing, fire resistance, healing (when hurt) and swiftness (with a target), then
    `nextFloat() < 7.5E-4` for its particles' entity event 15; a witch hurt by a player drinks
    healing: a potion in hand, `witch_drink`, slowed by 0.25 for the potion's use time. Mob
    effects are missing (1.6, 3.6).
18. **Every `Raider` and the witch draw the patrol leader's 0.06** at every spawn but
    `PATROL`, `EVENT` and `STRUCTURE` (PatrollingMonster.txt 99–150), before asking whether
    the type can lead; a pillager or vindicator leader wears the ominous banner, a banner
    with patterns, which needs item components.
19. **The guardian's spikes hurt**: a guardian that is not moving hurts a living direct
    attacker by `thorns` 2 (Guardian.txt 831–874); a creative player is not hurt. On land it
    flops with three own draws a tick on the ground (680–743). The elder guardian curses
    survival players within 50 every 1200 ticks with mining fatigue III (ElderGuardian.txt
    `customServerAiStep`; `MobEffectUtil.addEffectToPlayersAround` keeps survival players
    only): nothing for a creative player.
20. **The zombie family's conversions are timers on the body**: a zombie (and a husk) under
    water counts `InWaterTime` to 600 and `DrownedConversionTime` 300 more, then becomes a
    drowned (a zombie for a husk) (Zombie.txt 45–80, 405–421; `ConversionTracker` not dumped);
    a zombie villager's cure needs weakness, a mob effect (ZombieVillager.txt 277–318). A
    zombie standing in a pond in N7 would convert in 45 seconds.

**Dumped on 2026-10-21 (section 11)**

21. **The skeleton family, the creeper, the spiders and the drowned were dumped on
    2026-10-21**, with `ConversionTracker`, `BlockPattern`, `Crackiness`, the enchantment
    providers and the snowball (`n7k-javap/`, 59 classes); section 11 reads them and
    supersedes 3.4.4, 3.5, 3.9's rows for them and section 5's "not dumped" row. What is
    still missing is section 9's.
22. **Only the creeper and the spiders float**: the skeleton family has no `FloatGoal` and
    sinks as the zombies do (11.2). **The drowned swims only at night** (its swim-up goal) or
    after a target in water; by day it walks to water from land (up to 30 own draws each
    time the goal is asked) and in water sinks and walks the bottom (11.1).
23. **Monsters act without a target in ordinary play**, each with animals Clustine has:
    skeletons run from wolves within 6 and, burning in daylight, flee to shade and keep their
    paths out of the sun (11.2); creepers run from cats and ocelots (11.3); spiders from
    armadillos that are not rolled up (11.4); **every zombie, husk, zombie villager and
    drowned walks to turtle eggs within 24 and tramples them**, drawing `nextInt(200)` about
    every 100 to 200 ticks when none is near (11.8).
24. **A creeper explodes in ordinary play**: flint and steel or a fire charge on it (a
    player's use) ignites it, and 30 ticks later it explodes with radius 3 and the `MOB`
    interaction (blocks only with `mob_griefing`); Clustine has explosions. A fall swells it
    only on the server, harmlessly (11.3).
25. **The drowned's equipment ignores the difficulty**: `nextFloat() > 0.9` then
    `nextInt(16)`: a trident 0.0625, a fishing rod 0.0375 at easy, and a nautilus shell 0.03
    in the off hand (a sure drop); no armour draws (its `populateDefaultEquipmentSlots` does
    not call `Monster`'s). For `NATURAL` and `STRUCTURE` (ocean ruins) half the adult trident
    holders outside rivers ride a new zombie nautilus (11.1).
26. **The cave spider's `finalizeSpawn` draws nothing**, not even the Mob tail; the spider's
    effect is drawn **only on hard**, its skeleton jockey (one in 100) at any difficulty and
    added as a passenger by the caller (11.4).
27. **`Mob.convertTo` calls no `finalizeSpawn` and copies no health**: the zombie's
    conversion runs `handleAttributes(…, CONVERSION)` on the new mob (four own draws), the
    skeleton's nothing; equipment, the passenger, the vehicle, the leash, effects, the baby
    flag and more are copied (11.5). A zombie converts 600 + 301 ticks after its eye goes
    under (601 + 301 once it has left water); a skeleton 140 + 301 ticks in powder snow;
    leaving resets both.
28. **`BlockPattern.find` tries all 24 orientations** at 27 corner cells in a fixed order,
    upside down and lying down included (11.7). The `~` predicate is a method reference whose
    target this dump cannot show (**Inferred**: air).
29. **The golem's crack levels** are below 0.75, 0.5 and 0.25 of its health, strict (11.6);
    **building the enchantment providers** needs the enchantment registry's data, item
    enchantability and enchantments on item stacks (11.9); a **snowball** hurts for 0 and
    knocks back, which is the snow golem's defence (11.10).

---------------------------------------------------------------------------------------

## 1. The golems

### 1.0 Types

| Type | Id | Box (w × h), eye | Category, tracking | Attributes (`DEFAULT_ATTRIBUTES`; the class's lines) |
|---|---|---|---|---|
| `iron_golem` | 71 | 1.4 × 2.7, 2.295 | misc, 10 chunks, every 3 ticks | max health 100, speed 0.25, knockback resistance 1, attack damage 15, step height 1, follow range 16 (Mob's) (IronGolem.txt 163–182) |
| `snow_golem` | 124 | 0.7 × 1.9, 1.7 | misc, 8 chunks, every 3 ticks | max health 4, speed 0.2 (`0.20000000298023224`), follow range 16 (SnowGolem.txt 73–83) |

### 1.1 `AbstractGolem` (AbstractGolem.txt, 35 lines)

A `PathfinderMob`. Ambient, hurt and death sounds null; ambient interval 120 (Mob's
`nextInt(1000)` draw for the ambient sound is still made every tick, B 1.3, and nothing is
played for a null sound); `removeWhenFarAway` false: a golem never despawns, persistent or
not.

### 1.2 `IronGolem` (IronGolem.txt, 776 lines)

`extends AbstractGolem implements NeutralMob`.

- **Synched** (`defineSynchedData`, 150–162; static 765–776): `DATA_FLAGS_ID`, a `BYTE`,
  default 0; mask 1 is player-created (`isPlayerCreated`, 615–630; `setPlayerCreated`,
  631–662, sets or clears that bit only). Data index 16 (PathfinderMob and AbstractGolem add
  none; Mob's last is 15, B 1.15).
- **Saved** (284–316): `PlayerCreated` (boolean, read with default false), then
  `NeutralMob.addPersistentAngerSaveData` (`anger_end_time`, `angry_at`; read by
  `readPersistentAngerSaveData`, B 6.3.2). Not saved: `attackAnimationTick`,
  `offerFlowerTick`.
- **`PERSISTENT_ANGER_TIME`** `TimeUtil.rangeOfSeconds(20, 39)`, 400 to 780 ticks
  (static); `startPersistentAngerTimer` (317–326) samples it from the own random.
- **`aiStep`** (210–243): `AbstractGolem.aiStep` (the body's); `attackAnimationTick` and
  `offerFlowerTick` each count down to 0; on the server `updatePersistentAnger(level, true)`
  (B 6.3.4, as the polar bear's; Clustine's `types/neutral.rs`).
- **`decreaseAirSupply(air)`** returns `air` (183–187): it never drowns.
- **`doPush(entity)`** (188–209): if the pushed entity is an `Enemy` and not a `Creeper`,
  `getRandom().nextInt(20) == 0` (own random, drawn only then) makes it the golem's target
  (`setTarget`, no goal involved); then `AbstractGolem.doPush`.
- **`canSpawnSprintParticle`** (244–261): `horizontalDistanceSqr(motion) >
  2.500000277905201E-7` and `random.nextInt(5) == 0`. Called from `Entity.baseTick` on
  both sides (n6 Entity.txt 963–1000); when true, `spawnSprintParticle` (n6 Entity.txt
  4624–) reads the block under the feet and, unless its render shape is invisible, draws
  `nextDouble()` twice for the particle's offsets. The draws are the server's too; the
  particle is the client's.
- **`canAttack(target)`** (262–283): never a player when player-created; never a creeper;
  else `AbstractGolem.canAttack` (the body's).
- **`doHurtTarget(level, target)`** (362–442), in order:
  1. `attackAnimationTick = 10`; `level.broadcastEntityEvent(this, 4)`.
  2. `d = (float) getAttributeValue(ATTACK_DAMAGE)` (354–360); `amount = (int) d > 0 ? d /
     2 + random.nextInt((int) d) : d` (floats; one own draw; 15 gives 7.5 to 21.5).
  3. `source = damageSources().mobAttack(this)`; `hurt = target.hurtServer(level, source,
     amount)` (the target's own knockback from the source happens there, B 1.4).
  4. If hurt: `kr` is the target's `KNOCKBACK_RESISTANCE` if it is living, else 0;
     `target.setDeltaMovement(target.getDeltaMovement().add(0, 0.4000000059604645 ×
     max(0, 1 − kr), 0))`; `EnchantmentHelper.doPostAttackEffects(level, target, source)`
     (none without enchantments).
  5. Either way `playSound(IRON_GOLEM_ATTACK, 1, 1)`. Return `hurt`.
- **`hurtServer`** (443–467): the crack level before (`getCrackiness`, 468–478:
  `Crackiness.GOLEM.byFraction(health / maxHealth)`); `AbstractGolem.hurtServer`; if hurt
  and the level changed, `playSound(IRON_GOLEM_DAMAGE, 1, 1)`. `Crackiness` is not dumped;
  its four levels and thresholds (**Inferred**: none above 0.75, low, medium, high below
  0.25) decide only that sound. The client draws the cracks from the health (**Client**).
- **`handleEntityEvent`** (479–511, the client's): 4 the attack animation (10) and the
  attack sound; 11 `offerFlowerTick = 400`; 34 `offerFlowerTick = 0`.
- **`offerFlower(boolean)`** (518–540): true: 400 and entity event 11; false: 0 and 34.
- **`mobInteract(player, hand)`** (551–599), the repair:
  ```
  stack = player.getItemInHand(hand)
  if !stack.is(IRON_INGOT): return PASS
  before = getHealth(); heal(25.0f)
  if getHealth() == before: return PASS                      // at full health
  pitch = 1.0f + (random.nextFloat() - random.nextFloat()) * 0.2f   // two own draws
  playSound(IRON_GOLEM_REPAIR, 1.0f, pitch)
  stack.consume(1, player)                                   // kept by infinite materials
  return SUCCESS
  ```
  No `ENTITY_INTERACT` game event of its own beyond `Mob.interact`'s (B 1.10.2).
- **Sounds**: hurt `iron_golem_hurt`, death `iron_golem_death` (541–549), step
  `iron_golem_step` at 1, 1 (600–607), none ambient; attack, damage and repair above.
- **`checkSpawnObstruction(level)`** (670–733): the block below `entityCanStandOn`; the
  two cells above the golem's block valid empty spawn blocks for an iron golem, and its own
  block valid with an empty fluid; and `isUnobstructed`. Used by a villager's summoning
  (N10); building does not ask it.
- **`getLeashOffset`** (734–751): `(0, (double)(0.875f × eyeHeight), (double)(0.4f ×
  width))`.
- **`die`** (663–668): `AbstractGolem.die` only. Loot `entities/iron_golem`: poppies and
  iron ingots (two pools).

### 1.3 The iron golem's goals (B 8.15, checked)

`registerGoals` (27–149) is as B 8.15 has it: goals 1 `MeleeAttackGoal(1.0, true)`, 2
`MoveTowardsTargetGoal(0.9, 32.0f)`, 2 `MoveBackToVillageGoal(0.6, false)`, 4
`GolemRandomStrollInVillageGoal(0.6)`, 5 `OfferFlowerGoal`, 7 `LookAtPlayerGoal(Player,
6.0f)`, 8 `RandomLookAroundGoal`; targets 1 `DefendVillageTargetGoal`, 2
`HurtByTargetGoal()` (no exceptions), 3 `NearestAttackableTargetGoal(Player, 10, true,
false, selector #0)` (the anger's), 3 `NearestAttackableTargetGoal(Mob, 5, false, false,
lambda$registerGoals$0)` (752–763: an `Enemy` that is not a `Creeper`), 4
`ResetUniversalAngerTargetGoal(false)`. `OfferFlowerGoal` (read again): `canUse` needs
`level.isBrightOutside()`, then `nextInt(8000) == 0`, then the nearest of
`#candidate_for_iron_golem_gift` (villagers and `#accepts_iron_golem_gift`, the copper
golem) in the box grown by 6, 2, 6; `tick` looks at it (30, 30); `stop` gives a poppy only
to an `#accepts_iron_golem_gift` mob with an empty slot, as a guaranteed drop. A villager
is only looked at: no item moves.

What the goals do in N7k, with B 8.15's rules:

- **Strolling**: "anywhere" after one or two draws of the level's random, since no villager
  wants a golem (no brain memories) and no point of interest exists (B 8.15). B 7.14.2 says
  a golem without points of interest leaves its village; so does the game's when no
  villages' points exist, which is not the case in the game. Named.
- **Going back to the village**: `level.isVillage` is false everywhere: the goal's interval
  draw is made every tick it is asked and it never moves the golem (B 8.15).
- **Flowers**: in daylight a golem near a villager holds out a poppy for 400 ticks about
  once in 8000 asks: visible in a village within minutes.
- **Fighting**: the target goal against monsters fires within its follow range 16 once
  monsters' bodies exist (section 3); the melee goal walks to the target and calls
  `doHurtTarget`. Against a creative player never (B 2.0.1).
- **Defending the village**: needs reputations (N10): never.

### 1.4 `SnowGolem` (SnowGolem.txt, 522 lines)

`extends AbstractGolem implements RangedAttackMob, Shearable`.

- **Synched** (84–96; static 515–522): `DATA_PUMPKIN_ID`, a `BYTE`, index 16, default 16
  (`PUMPKIN_FLAG`, mask 0x10). `hasPumpkin` (399–414) tests the mask; `setPumpkin`
  (415–446) sets it or clears it with `& -17`.
- **Saved** (97–121): `Pumpkin` (boolean; read with default **true**).
- **Goals** (17–72): 1 `RangedAttackGoal(this, 1.25, 20, 10.0f)`; 2
  `WaterAvoidingRandomStrollGoal(1.0, 1.0000001E-5f)`; 3 `LookAtPlayerGoal(Player, 6.0f)`;
  4 `RandomLookAroundGoal`; target 1 `NearestAttackableTargetGoal(Mob, 10, true, false,
  lambda$registerGoals$0)` (509–514: any `Enemy`, creepers included).
- **`isSensitiveToWater`** true (122–126): `LivingEntity.aiStep` (LivingEntity.txt
  8590–8600) hurts it by `drown` 1 every tick it is in water or rain on the server (the
  invulnerable time makes that 1 every 10 ticks; 4 health is two seconds). Clustine has no
  rain.
- **`aiStep`** (127–247), after `AbstractGolem.aiStep`, on the server:
  ```
  if environmentAttributes.getValue(SNOW_GOLEM_MELTS, position()):
      hurtServer(level, damageSources().onFire(), 1.0f)         // a hurt, not a fire
  if !gameRules.get(MOB_GRIEFING): return
  snow = SNOW.defaultBlockState()                               // one layer
  for i in 0..4:
      x = Mth.floor(getX() + (double)((float)((i % 2) * 2 - 1) * 0.25f))
      y = Mth.floor(getY())
      z = Mth.floor(getZ() + (double)((float)((i / 2 % 2) * 2 - 1) * 0.25f))
      p = (x, y, z)
      if level.getBlockState(p).isAir() && snow.canSurvive(level, p):
          level.setBlockAndUpdate(p, snow)
          level.gameEvent(BLOCK_PLACE, p, GameEvent.Context.of(this, snow))
  ```
  `snow_golem_melts` is set by the Nether's dimension type (`dimension_type/
  the_nether.json` 14) and, by B 8.15, by seven warm biomes (badlands, eroded badlands,
  wooded badlands, desert, savanna, savanna plateau, windswept savanna). The snow is placed
  whether it melts or not: `SnowLayerBlock.canSurvive` reads only the block below
  (`#cannot_support_snow_layer`, `#support_override_snow_layer`, a full top face, or a snow
  layer of 8), no temperature. Clustine has no game rules and takes `mob_griefing` as on.
- **`performRangedAttack(target, power)`** (248–323): `dx = target.getX() − getX()`, `dy =
  target.getEyeY() − 1.100000023841858`, `dz = target.getZ() − getZ()`, `h =
  Math.sqrt(dx² + dz²) × 0.20000000298023224` (doubles); on a server, a `Snowball` made with
  a snowball stack and shot by `Projectile.spawnProjectile` with the lambda (491–508):
  `shoot(dx, dy + h − snowball.getY(), dz, 1.6f, rangedAttackUncertainty(level))`, the
  uncertainty 12 (324–328); then `playSound(SNOW_GOLEM_SHOOT, 1.0f, 0.4f /
  (getRandom().nextFloat() × 0.4f + 0.8f))`. The snowball's own rules are not dumped.
- **`mobInteract`** (329–370): shears and `readyForShearing()` (a pumpkin, 393–398): on the
  server `shear(level, PLAYERS, stack)`, game event `SHEAR` (the player), `stack.hurtAndBreak(1,
  player, hand's slot)`; `SUCCESS` on both sides; else `PASS`.
- **`shear(level, source, stack)`** (371–392): `level.playSound(null, this,
  SNOW_GOLEM_SHEAR, source, 1, 1)`; `setPumpkin(false)`; `dropFromShearingLootTable(level,
  shearing/snow_golem, stack, …)` with each drop `spawnAtLocation` at the eye height
  (480–490). The table drops a carved pumpkin.
- **Sounds** (447–461): ambient `snow_golem_ambient`, hurt `snow_golem_hurt`, death
  `snow_golem_death`; shoot and shear above. Loot `entities/snow_golem`: snowballs.
- **`getLeashOffset`** (462–479): `(0, (double)(0.75f × eyeHeight), (double)(0.4f ×
  width))`.

### 1.5 Building from a carved pumpkin (CarvedPumpkinBlock.txt, 661 lines)

- **`onPlace(state, level, pos, old, moved)`** (36–49): if `old` is not the same block,
  `trySpawnGolem(level, pos)`. A carved pumpkin and a jack o'lantern both are this class
  (**Inferred**: the jack o'lantern's block class; `PUMPKINS_PREDICATE`, 627–641, accepts
  both).
- **`canSpawnGolem(level, pos)`** (50–74): any of the snow, iron or copper golem's *base*
  pattern found at `pos`: the dispenser's question before it places a pumpkin (n6's
  dispenser behaviour, **Inferred**).
- **The patterns** (`BlockPatternBuilder.aisle`, rows from the top; 367–512):

  | Pattern | Rows | Keys |
  |---|---|---|
  | snow golem, base | `" "`, `"#"`, `"#"` | `#` a snow block; space anything |
  | snow golem, full | `"^"`, `"#"`, `"#"` | `^` a carved pumpkin or jack o'lantern |
  | iron golem, base | `"~ ~"`, `"###"`, `"~#~"` | `#` an iron block; `~` lambda #2 (**Inferred**: air) |
  | iron golem, full | `"~^~"`, `"###"`, `"~#~"` | as above |

- **`trySpawnGolem(level, pos)`** (75–170), in order: the snow golem's full pattern; else
  the iron golem's; else the copper golem's. For a match: `type.create(level, TRIGGERED)`;
  the iron golem `setPlayerCreated(true)`; `spawnGolemInWorld(level, match, golem,
  match.getBlock(c, 2, 0).getPos())` with `c` 0 for the snow golem and 1 for the iron golem
  (the bottom cell of the middle column); the copper golem's chest after.
- **`spawnGolemInWorld`** (215–268): `clearPatternBlocks` (269–310: every cell of the
  match's width and height, row by row, set to air with flags 2 and level event 2001 with
  the old state's id); `golem.snapTo(x + 0.5, y + 0.05, z + 0.5, 0, 0)`;
  `level.addFreshEntity(golem)`; `SUMMONED_ENTITY` for each server player within 5 of its
  box; `updatePatternBlocks` (311–341: `updateNeighborsAt(cell, AIR)` for every cell). No
  `finalizeSpawn` and no sound.
- **`BlockPattern.find`** is not dumped: which of the orientations it tries (the game builds
  golems lying down, **Inferred**) and in which order is not read here. Clustine's copper
  golem builds the upright form only and names it (`region/copper_golems.rs` 15–16); the
  same limit for these two would be a named difference a player meets only by building
  sideways.

### 1.6 What the golems need that Clustine lacks, and how to stub it

| Need | Where | Stub that keeps ordinary play |
|---|---|---|
| A type's own `doHurtTarget` | iron golem's hit and fling | a hook in `melee_hit`; the fling a field of the hurt (section 7) |
| `MoveTowardsTargetGoal`, `MoveBackToVillageGoal`, `GolemRandomStrollInVillageGoal`, `OfferFlowerGoal`, `DefendVillageTargetGoal` | 1.3 | port the first three and the flower; the defence never fires without reputations: an empty goal that keeps its slot |
| Points of interest, villages | stroll, going back | none exist: "anywhere" (B 8.15), named |
| Projectiles (the snowball) | snow golem's attack | the shoot sound and no snowball, as the llama's spit is (`types/llama.rs` 532–618), named; or a snowball entity with N9's projectiles (section 10) |
| Environment attribute `snow_golem_melts` | snow golem's `aiStep` | the seven biomes as a sorted list, as `random_ticks/fire.rs` does for `increased_fire_burnout` |
| Game rule `mob_griefing` | snow trail | on, as everywhere in Clustine |
| `shearing/snow_golem` loot table | shearing | extract it (only `shearing/sheep` and `shearing/mooshroom` are in `loot_tables.rs` today) |
| Rain | water hurt | none until weather |
| `BlockPattern` | building | hand-written upright shapes, as the copper golem's |
| Mob effects | none of the golems' | — |

---------------------------------------------------------------------------------------

## 2. Villagers and wandering traders, as bodies

### 2.0 Types

| Type | Id | Box, eye | Category, tracking | Attributes |
|---|---|---|---|---|
| `villager` | 143 | 0.6 × 1.95, 1.62 | misc, 10 chunks | speed 0.5 (Villager.txt 182–189), follow range 16, health 20 |
| `wandering_trader` | 145 | 0.6 × 1.95, 1.62 | creature, 10 chunks | Mob's (no `createAttributes` of its own): speed 0.7, follow range 16, health 20 |

Baby dimensions are each class's `BABY_DIMENSIONS` (Villager.txt 413–424; the values are in
the static block, not quoted here).

### 2.1 `VillagerData` (VillagerData.txt, 277 lines), types and professions

- A record `(Holder<VillagerType> type, Holder<VillagerProfession> profession, int level)`;
  levels 1 to 5, the experience thresholds `{0, 10, 70, 150, 250}` (static 235–277).
- **Codec** (170–234): `type` (default `plains`), `profession` (default `none`), `level`
  (default 1), each an optional field. **Stream codec**: type and profession as holders
  (registry ids, var-ints), level as a var-int (`ByteBufCodecs.VAR_INT`).
- **The synched-data serializer** `VILLAGER_DATA` is wire id **18** (the order of
  `registerSerializer` in EntityDataSerializers.txt: 0 byte, 1 int, 2 long, 3 float, 4
  string, 5 component, 6 optional component, 7 item stack, 8 boolean, …, 14 block state, 15
  optional block state, 16 particle, 17 particles, 18 villager data, 19 optional unsigned
  int, 20 pose, …).
- **`Villager.createDefaultVillagerData()`** (768–781): `plains`, `none`, 1.
- **`VillagerDataHolder.finalizeVillagerType(level, pos)`** (VillagerDataHolder.txt): unless
  `getVillagerDataFinalized()`, `setVillagerData(data.withType(registryAccess,
  VillagerType.byBiome(level.getBiome(pos))))` and `setVillagerDataFinalized(true)`. No draw.
- **`VillagerType.byBiome`** (79–93, the map built in `lambda$static$0`, 94–242): desert for
  badlands, desert, eroded badlands, wooded badlands; jungle for bamboo jungle, jungle,
  sparse jungle; savanna for savanna plateau, savanna, windswept savanna; snow for deep
  frozen ocean, frozen ocean, frozen river, ice spikes, snowy beach, snowy taiga, snowy
  plains, grove, snowy slopes, frozen peaks, jagged peaks; swamp for swamp and mangrove
  swamp; taiga for old growth spruce taiga, old growth pine taiga, windswept gravelly hills,
  windswept hills, taiga, windswept forest; **plains for every other biome** (the map's
  `orElse`). Seven types: desert, jungle, plains, savanna, snow, swamp, taiga (1–16).
- **Professions** (VillagerProfession.txt 165–695): none, armorer, butcher, cartographer,
  cleric, farmer, fisherman, fletcher, leatherworker, librarian, mason, nitwit, shepherd,
  toolsmith, weaponsmith; `getTrades(level)` (696–704) is the level's trade set from
  `tradeSetsByLevel`, null where there is none. None and nitwit are registered by the
  overloads without trade sets (84–124; **Inferred**: their maps are empty). A profession's
  `workSound` is played by the brain's work (N10).

### 2.2 `AbstractVillager` and `Villager`: state, synched and saved

`Villager extends AbstractVillager extends AgeableMob`.

- **Construction** (AbstractVillager.txt 13–34; Villager.txt 59–101): an inventory of 8
  (`SimpleContainer`); path malus 16 for `FIRE_IN_NEIGHBOR`, −1 for `FIRE`; the
  villager's navigation opens doors, floats (`setCanFloat`: what a path may cross, not the
  body's rising) and has a required path length 48; `setCanPickUpLoot(true)`. No draw of
  its own (the brain's sensors draw their start delays, B 7.1).
- **Synched**, after `AgeableMob`'s 16 (baby) and 17 (age locked):

  | Index | Accessor | Serializer | Default |
  |---|---|---|---|
  | 18 | `AbstractVillager.DATA_UNHAPPY_COUNTER` | `INT` (1) | 0 |
  | 19 | `Villager.DATA_VILLAGER_DATA` | `VILLAGER_DATA` (18) | plains, none, 1 |
  | 20 | `Villager.DATA_VILLAGER_DATA_FINALIZED` | `BOOLEAN` (8) | false |

  The wandering trader has 16 to 18 only.
- **Saved**: `AbstractVillager` (238–279) `Offers` (when there are any) and the inventory
  (`writeInventoryToTag`, `Inventory`); `Villager` (782–831) `VillagerData`,
  `VillagerDataFinalized`, `FoodLevel` (byte), `Gossips`, `Xp`, `LastRestock`,
  `LastGossipDecay`, `RestocksToday`; the brain's memories are `Brain`'s (B 7.1).
- **Read** (832–920): `VillagerDataFinalized` true, **or a `VillagerData` present**, marks
  it finalised; the data read, or the default; the rest with defaults 0; then on a server
  `refreshBrain` (121–141: stop every behaviour, pack the memories, a new brain, the
  schedule set; B 7.14.1).
- **`removeWhenFarAway`** false (933–937).
- **`setVillagerData(data)`** (975–995): a change of profession clears the offers.

### 2.3 `finalizeSpawn`, by reason

`Villager.finalizeSpawn` (1440–1465): if the reason is `BREEDING`, the profession set to
none; `finalizeVillagerType(level, blockPosition())`; then `AbstractVillager.finalizeSpawn`
(35–51): a null group becomes `AgeableMobGroupData(false)` (no babies by the group), then
`AgeableMob.finalizeSpawn` and the Mob tail (B 8.7: the follow-range triangle unless the
bonus is there, `nextDouble()` twice, then `nextFloat()` for the left hand). So:

| Reason | What it does | Draws (the level's random) |
|---|---|---|
| `SPAWN_ITEM_USE` (an egg on a block or water) | type by biome, finalised; unemployed, level 1, adult | triangle, hand |
| `STRUCTURE` (village templates) | type kept (the template's `VillagerData` finalises it at the load); profession and age the template's | triangle (the village templates carry no bonus), hand (B 8.12) |
| `NATURAL`, `CHUNK_GENERATION` | not made by either for a villager (it has no spawn list entry; villages make them) | — |
| `BREEDING` | N10's; profession none | — |
| `CONVERSION` (a cured zombie villager) | N9's | — |

The wandering trader has no `finalizeSpawn` of its own: `AbstractVillager`'s, so an egg's
trader draws the triangle and the hand.

**An egg on a villager** (`Mob.interact` → `spawnOffspringFromSpawnEgg`, B 1.11.4):
`getBreedOffspring(level, itself)` (Villager.txt 1466–1521): one `random.nextDouble()` of
**its own** random, `< 0.5` the type by the biome at its feet, `< 0.75` its own type, else
the partner's (itself); a new villager with that type, unemployed, finalised; made a baby
(`setBaby(true)`), placed at the parent's feet facing south, no `finalizeSpawn`. An egg on a
wandering trader makes nothing (`getBreedOffspring` null, WanderingTrader.txt 220–224).

### 2.4 What a villager does with no work

`Villager.customServerAiStep` (190–261), each tick on the server: `brain.tick` first;
then the trading reputation event (none without trades); then, unless `NoAI`,
**`random.nextInt(100)`** (own) and only if it is 0 the raid lookup at its position (none:
entity event 42 never); then stopping a trade with no offers; then
`AbstractVillager.customServerAiStep`. `tick` (262–278): the unhappy counter counts down to
0 (synched: it shakes its head while above 0, **Client**), and `maybeDecayGossip` (game
time, gossips: nothing to decay in N7). The body's: falls, is pushed and pushes, walks
where its brain's walk target says, picks up wanted items (finding 14), takes damage.

What it does in N7k depends on the brain it is given (P 11.2 "villagers with the core and
idle activities and their schedule's rest"; P 11.7 question 5 "wander by day and rest at
night"; L 1.2 "standing, falling, floating and resting as N7k's reduced villager does"):

- **No brain** (or `NoAI`): stands, falls, is pushed; in water it **sinks** and drowns
  (air 300, the body's rule), since floating is the core package's `Swim(0.8f)` (B 7.14.1).
  The `nextInt(100)` is still drawn unless `NoAI`.
- **The reduced brain of B 7.14.2**: `CORE` = 0 `Swim(0.8f)`, 0 `InteractWithDoor`, 0
  `LookAtTargetSink(45, 90)`, 1 `MoveToTargetSink()`; `IDLE` = 2 `RunOne`
  [`SetWalkTargetFromLookTarget(0.5, 2)` 1, a stroll 1, `DoNothing(30, 60)` 1], 5 the full
  look `RunOne`; sensors `NEAREST_LIVING_ENTITIES`, `NEAREST_PLAYERS`. All of these exist
  in Clustine since N7j (section 8). The game's stroll is `VillageBoundRandomStroll(0.5)`,
  which falls back to `LandRandomPos.getPos(mob, 10, 7)` when no village section is near:
  without points of interest that is what the game's would do too, so it leaves its
  village (B 7.14.2).
- **The schedule's rest**: `REST` is the schedule's at night by the environment attribute
  `VILLAGER_ACTIVITY` (`timeline/villager_schedule.json`); its package sleeps in a bed found
  by points of interest (N10). Without beds, resting is "the idle package does not run":
  the villager stands and looks. **Inferred**: what the game's villager does at night with
  no bed is the rest package's own behaviours, not read here (section 10).
- A baby's `PLAY` package (`BABY_VILLAGER_ACTIVITY`) is not in B 7.14.2's reduction:
  babies would idle as adults do.

### 2.5 A player's use of a villager

`Villager.mobInteract(player, hand)` (279–337), after `Mob.interact`'s name tag, egg and
leash (B 1.10.2):

```
stack = player.getItemInHand(hand)
if !stack.is(VILLAGER_SPAWN_EGG) && isAlive() && !isTrading() && !isSleeping():
    if isBaby(): setUnhappy(); return SUCCESS
    if !client:
        empty = getOffers().isEmpty()          // updateTrades if none yet (AbstractVillager.txt 113–144)
        if hand == MAIN_HAND:
            if empty: setUnhappy()
            player.awardStat(TALKED_TO_VILLAGER)
        if empty: return CONSUME
        startTrading(player)                   // the merchant screen (352–369)
    return SUCCESS
return AbstractVillager.mobInteract(player, hand)   // AgeableMob's: the golden dandelion
```

`setUnhappy` (338–351): unhappy counter 40 and, on the server, `makeSound(VILLAGER_NO)`
(Mob's volume and voice pitch, two own draws). `updateTrades` (1666–1708): the profession's
trade set for its level; none for unemployed and nitwits, so the offers stay empty. So in
N7k every villager a player meets shakes its head and says no, as an unemployed one does
in the game; the off hand awards nothing and plays nothing. The igloo's cleric keeps its
template's `Offers` (L 1.2): it is the one villager whose click would open a screen (L
section 8, question 7).

### 2.6 Sounds and entity events

- Ambient (938–952): none while sleeping, `villager_trade` while trading, else
  `villager_ambient`; hurt `villager_hurt`, death `villager_death`; `villager_no` above;
  the trade sounds `villager_yes`/`villager_no` and the celebrate sound are trading's and
  raids'.
- Entity events (`handleEntityEvent`, 1405–1439, the client's): 12 hearts (breeding), 13
  angry particles, 14 happy particles (a trade), 42 splashes (a raid). None is sent in N7k.

### 2.7 The wandering trader (WanderingTrader.txt, 541 lines)

- **Goals** (20–219): 0 `FloatGoal`; 0 `UseItemGoal` (an invisibility potion,
  `wandering_trader_disappeared`, lambda #0: **Inferred** at night and not invisible); 0
  `UseItemGoal` (milk, `wandering_trader_reappeared`, lambda #1: **Inferred** by day and
  invisible); 1 `TradeWithPlayerGoal`; 1 `AvoidEntityGoal` of zombies (8), evokers (12),
  vindicators (8), vexes (8), pillagers (15), illusioners (12), zoglins (10), each at 0.5,
  0.5; 1 `PanicGoal(0.5)`; 1 `LookAtTradingPlayerGoal`; 2 `WanderToPositionGoal(2.0,
  0.35)`; 4 `MoveTowardsRestrictionGoal(0.35)`; 8 `WaterAvoidingRandomStrollGoal(0.35)`; 9
  `InteractGoal(Player, 3, 1)`; 10 `LookAtPlayerGoal(Mob, 8)`. A goal-driven mob, not a
  brain's: Clustine has `FloatGoal`, `UseItemGoal`, `AvoidEntityGoal`, `PanicGoal`,
  `MoveTowardsRestrictionGoal` and the look goals (section 8).
- **Saved** (304–349): `DespawnDelay` (int), `wander_target` (a block position, optional);
  read: the age raised to at least 0.
- **`aiStep`** (445–476): `maybeDespawn`: a delay above 0 counts down while not trading and
  discards the trader at 0; an egg's trader has 0 and never despawns (the spawner's sets it,
  N9). `removeWhenFarAway` false.
- **`mobInteract`** (230–281): not with a villager egg, alive, not trading, not a baby: the
  main hand awards `TALKED_TO_VILLAGER`; on the server, empty offers → `CONSUME`; else
  `setTradingPlayer(player)`, `openTradingScreen(player, name, 1)`, `SUCCESS_SERVER`.
  `updateTrades` (282–303) fills offers from three trade sets (buying, uncommon, common):
  never empty, so the game always opens the screen.
- **Sounds** (387–431): ambient `wandering_trader_trade` while trading, else
  `wandering_trader_ambient`; hurt, death; drinking milk or a potion; yes and no.
- The invisibility potion and milk are mob effects; the trader's llamas are the spawner's
  (N9; Clustine's trader llama exists, `types/trader_llama.rs`).

---------------------------------------------------------------------------------------

## 3. The monsters' bodies

### 3.0 Types (from the extraction: `entity_types.rs`, `attributes.rs`)

Every one is of the category `monster`, tracked 8 chunks every 3 ticks (the elder guardian
10), and not fire-immune. Attributes are the type's base values where they differ from the
attribute's default (health 20, speed 0.7, follow range 32, armour 0, attack damage 2 for a
`Monster`, step height 0.6).

| Type | Id | Box, eye | Attributes | Dumped |
|---|---|---|---|---|
| zombie | 154 | 0.6 × 1.95, 1.74 | follow 35, speed 0.23, attack 3, armour 2, spawn reinforcements 0 | yes (Zombie.txt 232–250) |
| husk | 68 | 0.6 × 1.95, 1.74 | as the zombie | yes |
| drowned | 39 | 0.6 × 1.95, 1.74 | as the zombie, step height 1 | **no** |
| zombie villager | 157 | 0.6 × 1.95, 1.74 | as the zombie | yes |
| skeleton | 118 | 0.6 × 1.99, 1.74 | follow 16, speed 0.25 | **no** |
| stray | 131 | 0.6 × 1.99, 1.74 | follow 16, speed 0.25 | **no** |
| bogged | 16 | 0.6 × 1.99, 1.74 | follow 16, speed 0.25, health 16 | **no** |
| parched (26.3, a husk's rider) | 98 | 0.6 × 1.99, 1.74 | follow 16, speed 0.25, health 16 | **no** |
| creeper | 32 | 0.6 × 1.7, 1.445 | follow 16, speed 0.25 | **no** |
| spider | 127 | 1.4 × 0.9, 0.65 | follow 16, speed 0.3, health 16 | **no** |
| cave spider | 22 | 0.7 × 0.5, 0.45 | follow 16, speed 0.3, health 12 | **no** |
| witch | 148 | 0.6 × 1.95, 1.62 | follow 16, speed 0.25, health 26 (Witch.txt 170–180) | yes |
| pillager | 104 | 0.6 × 1.95, 1.658 | follow 32, speed 0.35, health 24, attack 5 (Pillager.txt 147–163) | yes |
| vindicator | 144 | 0.6 × 1.95, 1.658 | follow 12, speed 0.35, health 24, attack 5 (Vindicator.txt 191–207) | yes |
| evoker | 47 | 0.6 × 1.95, 1.658 | follow 12, speed 0.5, health 24 (Evoker.txt 167–180) | yes |
| guardian | 64 | 0.85 × 0.85, 0.425 | follow 16, speed 0.5, health 30, attack 6 (Guardian.txt 150–163) | yes |
| elder guardian | 41 | 1.997 × 1.997, 0.9987 | follow 16, speed 0.3, health 80, attack 8 (ElderGuardian.txt) | yes |

Tags (`data/minecraft/tags/entity_type/`): `#zombies` zombie horse, camel husk, zombie,
zombie villager, zombified piglin, zoglin, drowned, husk, zombie nautilus; `#skeletons`
skeleton, stray, wither skeleton, skeleton horse, bogged, parched; `#undead` both and the
wither and phantom; `#illager` evoker, illusioner, pillager, vindicator; `#raiders` evoker,
pillager, ravager, vindicator, illusioner, witch; `#burn_in_daylight` finding 1.

### 3.1 The layers they share

- **`Monster`** (Monster.txt, 241 lines): experience reward 5 (constructor, 3–13); sound
  source `HOSTILE` (14–18); `aiStep` (19–26) calls `updateNoActionTime` (27–43:
  `noActionTime += 2` where the light's magic value is above 0.5) then `PathfinderMob`'s;
  swim, splash, hurt and death sounds `hostile_*`, fall sounds `hostile_small_fall` and
  `hostile_big_fall` (44–72); walk target value `−(magic value − 0.5)` (73–80): darker is
  better; `createMonsterAttributes` is Mob's with attack damage (187–193); drops experience
  (194); loot only with the game rule `mob_drops` (199–208); **`isPreventingPlayerRest`
  true** (209–213: a monster within 8, 5, 8 of a bed refuses a sleeper who is not
  creative, n7d-brief 8, line 207); `getProjectile` an arrow for a bow (214–241). The spawn
  rules (81–186) are N9's.
- **`PatrollingMonster`** (267 lines): saved `patrol_target` (optional block position),
  `PatrolLeader`, `Patrolling` (43–93); `finalizeSpawn` (99–150): unless the reason is
  `PATROL`, `EVENT` or `STRUCTURE`, `level.getRandom().nextFloat() < 0.06f && canBeLeader()`
  makes it the patrol leader; a leader gets `Raid.getOminousBannerInstance` on its head with
  drop chance 2.0; `PATROL` sets patrolling; then `Monster`'s (the Mob tail). The 0.06 is
  drawn before `canBeLeader` is asked. `removeWhenFarAway` (169–182): **Inferred** `!patrolling
  || distance² > 16384`. Goal 4 `LongDistancePatrolGoal(0.7, 0.595)` (27–42).
- **`Raider`** (699 lines): synched `IS_CELEBRATING` boolean (75–87); saved `Wave`,
  `CanJoinRaid`, `RaidId` (357–428); `finalizeSpawn` (574–594): `setCanJoinRaid(!(witch &&
  NATURAL))`, then `PatrollingMonster`'s; `aiStep` (103–171): raid joining every 20 game
  ticks where a raid is (none in N7), `noActionTime = 0` with a player or iron golem as
  target; `updateNoActionTime` (172–181) adds 2 always; goals 1 `ObtainRaidLeaderBannerGoal`,
  3 `PathfindToRaidGoal`, 4 `RaiderMoveThroughVillageGoal(1.05, true)`, 5
  `RaiderCelebration` (33–74): all raid's (N10).
- **`AbstractIllager`** (61 lines): arm pose `CROSSED`; `canAttack` refuses a baby
  villager; allies with `#illager_friends` without teams.

### 3.2 Equipment and its enchantment at spawn (Mob.txt 2347–2644)

**`populateDefaultEquipmentSlots(r, difficulty)`** (2347–2429), with `s` the special
multiplier:

```
if r.nextFloat() < 0.15f * s:
    tier = r.nextInt(3)
    for k in 1..=3: if r.nextFloat() < 0.1087f: tier++      // three draws, always
    stop = level.getDifficulty() == HARD ? 0.1f : 0.25f
    first = true
    for slot in [HEAD, CHEST, LEGS, FEET]:                  // EQUIPMENT_POPULATION_ORDER, static 3695–3712
        item = getItemBySlot(slot)
        if !first && r.nextFloat() < stop: break
        first = false
        if item.isEmpty(): e = getEquipmentForSlot(slot, tier); if e != null: setItemSlot(slot, new ItemStack(e))
```

`getEquipmentForSlot(slot, tier)` (2430–2561): tier 0 leather, 1 **copper**, 2 golden, 3
chainmail, 4 iron, 5 diamond, for each armour slot (26.3 adds copper armour between leather
and gold).

**`populateDefaultEquipmentEnchantments(level, r, difficulty)`** (2562–2592):
`enchantSpawnedWeapon` (2593–2603: the main hand with chance 0.25) then, for every slot of
`EquipmentSlot.VALUES` whose type is humanoid armour, `enchantSpawnedArmor` (2604–2614:
chance 0.5). `enchantSpawnedEquipment(level, slot, r, chance, difficulty)` (2615–2644):
only where the slot holds something, `r.nextFloat() < chance × s` →
`EnchantmentHelper.enchantItemFromProvider(stack, registries,
VanillaEnchantmentProviders.MOB_SPAWN_EQUIPMENT, difficulty, r)` (EnchantmentHelper.txt
1332–1352). The provider `mob_spawn_equipment` is `by_cost_with_difficulty` over
`#on_mob_spawn_equipment` (= `#non_treasure`), min cost 5, cost span 17; its class is not
dumped, so its draws are not given here. At `s = 0` the draw is made and nothing is
enchanted.

`setDropChance(slot, f)` (2691–2714) sets a slot's chance; Clustine's default 0.085 is the
game's (`body.rs` 140).

### 3.3 Burning in daylight (Mob.txt 884–1088)

`Mob.aiStep` (884–979): `LivingEntity.aiStep` first; then, **if the type is in
`#burn_in_daylight`**, `burnUndead()`; then the loot pick-up (`canPickUpLoot`, alive, not
dead, `mob_griefing`: items in the box grown by `getPickupReach`, `wantsToPickUp`,
`pickUpItem`).

```
burnUndead():                                       // 985–1020
    if !isAlive() || !isSunBurnTick(): return
    slot = sunProtectionSlot()                      // HEAD (980–984); overridable
    item = getItemBySlot(slot)
    if !item.isEmpty():
        if item.isDamageableItem(): item.hurtAndBreak(random.nextInt(2), this, slot)
        return
    igniteForSeconds(8.0f)

isSunBurnTick():                                    // 1021–1088
    if client: return false
    if !environmentAttributes.getValue(MONSTERS_BURN, position()): return false
    b = getLightLevelDependentMagicValue()          // Entity's: at (x, eyeY, z), 0 where the chunk is not loaded
    pos = BlockPos.containing(getX(), getEyeY(), getZ())
    wet = isInWaterOrRain() || isInPowderSnow || wasInPowderSnow
    return b > 0.5f
        && random.nextFloat() * 30.0f < (b - 0.4f) * 2.0f     // own random; drawn whenever b > 0.5
        && !wet
        && level.canSeeSky(pos)
```

- **The time**: `gameplay/monsters_burn` (n7d EnvironmentAttributes.txt 556–567, default
  false) is set only by the `day` timeline in the overworld (`timeline/day.json` 90–102:
  keyframes 12542 false and 23460 true, modifier `or`; the overworld's timelines
  `#in_overworld` are `#universal`, `day`, `moon`, `early_game`). A boolean keyframe holds
  until the next (**Inferred**, as Clustine's `bees_stay_in_hive` reads the same timeline,
  `types/bee.rs` 323–327): monsters burn from tick 23460 of the day to 12542 of the next,
  by the overworld clock's day (`MobWorld::day`). Rain, which also wets, Clustine has not.
- **The light**: `LevelReader.getLightLevelDependentMagicValue(pos)` (LevelReader.txt
  211–236): `f = getMaxLocalRawBrightness(pos) / 15`, `v = f / (4 − 3f)`, `lerp(ambient,
  v, 1)` with the dimension's ambient light (0 in the overworld, so `v`).
  `getMaxLocalRawBrightness(pos)` (297–305) is `getRawBrightness(pos, getSkyDarken())`,
  15 outside ±30,000,000; `getRawBrightness` is `BlockAndTintGetter`'s, not dumped
  (**Inferred**: `max(sky − darken, block)`). `b > 0.5` is a raw brightness of 7 or more;
  at 15 the chance is `(1 − 0.4) × 2 / 30 = 0.04` a tick.
- **The sky**: `canSeeSky` is `BlockAndTintGetter`'s too (**Inferred**: the sky light at the
  position is 15; Clustine's fox has the same helper, `types/fox.rs` 286–290).
- **The fire**: `igniteForSeconds(8)` = `igniteForTicks(Mth.floor(8 × 20))` (n6 Entity.txt
  1271–1280); `LivingEntity.igniteForTicks(t)` (LivingEntity.txt 11078–11098) scales by the
  `burning_time` attribute (default 1) with `Mth.ceil`; `Entity.igniteForTicks` sets the
  fire ticks only if they are fewer, and clears freezing (1281–1290). The burning itself is
  the body's (Clustine `living.rs` 443–456: 1 `onFire` every 20 ticks; water puts it out,
  `living/inside.rs`).
- **Draws, in order, each tick in daylight and light**: `nextFloat()`; then, if it burns
  and wears a damageable helmet, `nextInt(2)`. A burning mob in the dark draws nothing for
  this.
- **Order in the tick**: after `LivingEntity.aiStep` (movement, pushing), before the loot
  pick-up and before `Monster.aiStep`'s caller returns (B 1.3's order: `Mob.aiStep` inside
  `LivingEntity.tick`).

### 3.4 The zombie family

#### 3.4.1 `Zombie` (Zombie.txt, 1456 lines)

- **Construction** (45–80): a `BreakDoorGoal` held for door breaking (difficulty hard,
  lambda 1389–1398); `canBreakDoors` false; a `ConversionTracker` for drowning with
  `DATA_DROWNED_CONVERSION_ID`, the condition `isEyeInFluid(WATER)` (1382–1388), the keys
  `InWaterTime` (600) and `DrownedConversionTime` (300), and on conversion
  `handleAttributes(special, CONVERSION)` on the new mob (1370–1381). `ConversionTracker`
  is not dumped (**Inferred**: in water with the eye under for 600 ticks starts a 300-tick
  conversion, shown by the synched flag, then `convertTo(convertsToWhenDrowning())`).
- **Synched** (251–275; static 1399–): 16 `DATA_BABY_ID` boolean false; 17
  `DATA_SPECIAL_TYPE_ID` int 0 (unused by any dumped method); 18 `DATA_DROWNED_CONVERSION_ID`
  boolean false.
- **Saved** (815–858): `IsBaby`, `CanBreakDoors`, then the tracker's keys; read with
  defaults false.
- **Baby** (334–404): `setBaby` sets the flag and, on the server, removes and (for a baby)
  adds the transient speed modifier `baby` +0.5 `ADD_MULTIPLIED_BASE`; dimensions
  `scalable(0.49, 0.98)`, eye 0.775, vehicle attachment (0, 0.1875, 0) (static 1440–1451);
  a baby's experience reward is `xpReward × 2.5` (344–361).
- **`setCanBreakDoors`** (289–333): only with ground navigation; adds or removes the break
  door goal at priority 1 and sets the navigation's door opening.
- **`tick`** (410–421): `Monster.tick`, then the drowning tracker if `convertsInWater()`
  (true for the zombie and the husk, false for the zombie villager).
- **`hurtServer`** (449–671): after `Monster.hurtServer`, reinforcements only on **hard**
  (`level.getDifficulty() == HARD`, a draw of the own random against
  `spawn_reinforcements`, then up to 50 tries): nothing below hard, no draw.
- **`doHurtTarget`** (672–711): after the hit, an empty-handed burning zombie sets the
  target alight for `2 × effective difficulty` seconds with `nextFloat() < 0.3 × effective`
  (N9's: no attack in N7).
- **Sounds** (712–751): `zombie_ambient`, `zombie_hurt`, `zombie_death`, step
  `zombie_step` at 0.15, 1; conversion level event 1040 (husk 1041).
- **`populateDefaultEquipmentSlots`** (763–814): `Monster`'s (3.2), then `r.nextFloat() <
  (hard ? 0.05f : 0.01f)` → `r.nextInt(6)`: 0 an iron sword, 1 an iron spear, else an
  iron shovel, in the main hand.
- **`finalizeSpawn`** (946–1141): section 5's order. `ZombieGroupData` and its constructor
  are not dumped; `getSpawnAsBabyOdds(r)` (1190–1201) is `r.nextFloat() < 0.05f`.
- **`handleAttributes(special, reason)`** (1202–1300), from the **own** random:
  1. `randomizeReinforcementsChance` (1301–1313): base of `spawn_reinforcements` =
     `nextDouble() × 0.10000000149011612`;
  2. `knockback_resistance` += `nextDouble() × 0.05000000074505806` (`random_spawn_bonus`,
     `ADD_VALUE`, permanent);
  3. `d = nextDouble() × 1.5 × special`; if `d > 1`: `follow_range` modifier
     `zombie_random_spawn_bonus` `d`, `ADD_MULTIPLIED_TOTAL`;
  4. if `nextFloat() < special × 0.05f`: the leader: `spawn_reinforcements` +
     `nextDouble() × 0.25 + 0.5` and `max_health` × `nextDouble() × 3 + 1`
     (`leader_zombie_bonus`, `ADD_VALUE` and `ADD_MULTIPLIED_TOTAL`); health to the maximum
     unless the reason is `CONVERSION`, `LOAD` or `DIMENSION_TRAVEL`; `setCanBreakDoors(true)`.
- **`onOffspringSpawnedFromEgg(player, child)`** (1142–1173): an egg's baby zombie gets
  `setCanPickUpLoot(parent.random.nextFloat() < 0.55f × special)`, the **parent's** own
  random, at the child's difficulty.
- **`killedEntity`** (859–900) and `convertVillagerToZombieVillager` (422–443, 1320–1369):
  a zombie killing a villager turns it (**Inferred**: on normal or hard; not read), N9's.

#### 3.4.2 `Husk` (Husk.txt, 286 lines)

Sounds `husk_*`; not sun-sensitive and not in `#burn_in_daylight`; `doHurtTarget` (38–77)
gives an empty-handed hit `hunger` for `140 × effective difficulty` ticks (a mob effect,
N9); converts in water to a **zombie** (78–92). **`finalizeSpawn`** (93–257): `Zombie`'s
first, then, unless `CONVERSION`, a **second** `r.nextFloat() < 0.55f × special` for the
loot; the group wrapped in `HuskGroupData` with `triedToSpawnCamelHusk = reason !=
NATURAL`; only for `NATURAL`, where a camel husk's box at the block's centre is free:
`r.nextFloat() < 0.1f` → an iron spear in hand, a camel husk made (`NATURAL`), placed,
finalised, ridden by the husk, and a parched made, finalised and riding the camel husk too
(N9's).

#### 3.4.3 `ZombieVillager` (ZombieVillager.txt, 899 lines)

- **Synched** (44–69; static 871–): 19 `DATA_CONVERTING_ID` boolean; 20
  `DATA_VILLAGER_DATA` (`VILLAGER_DATA`), whose default is
  `initializeZombieVillagerData(this.random)` (224–243: the default data with a profession
  drawn by `BuiltInRegistries.VILLAGER_PROFESSION.getRandom(random)`, **Inferred** one
  `nextInt(15)`): **a construction draw**; 21 `DATA_VILLAGER_DATA_FINALIZED` boolean.
- **Saved** (70–120): `VillagerData`, `VillagerDataFinalized`, `Offers`, `Gossips`,
  `ConversionTime` (−1 when not converting), `ConversionPlayer`, `Xp`. Read (121–208): a
  present `VillagerData` marks it finalised, as the villager's; a `ConversionTime` other
  than −1 restarts the conversion.
- **`finalizeSpawn`** (209–223): `finalizeVillagerType` (no draw), then `Zombie`'s.
- **`tick`** (244–276): a converting one counts down by `getConversionProgress()` (461–554,
  iron bars and beds near: reads blocks, **Inferred** draws) and becomes a villager at 0
  (`finishConversion`, 439–453, 737–852).
- **`mobInteract`** (277–318): a golden apple with **weakness** on it starts the cure
  (`3600 + nextInt(2401)` ticks, strength on it, entity event 16); without weakness,
  `Zombie`'s: nothing. Without mob effects nothing is ever weak: a golden apple does
  nothing, as on a zombie villager that is not weak in the game.
- `convertsInWater` false; `removeWhenFarAway` (324–336) only when not converting and with
  no experience; sounds `zombie_villager_*`, a baby's pitch (555–604).

#### 3.4.4 The drowned

`Drowned` is not dumped. It extends `Zombie` (its id in the burn tag and the zombies' tag;
**Inferred** the class chain); every rule of its own (swimming, the trident and nautilus
shell, `finalizeSpawn`, its synched and saved data) is to be read from a dump.

### 3.5 The skeleton family, the creeper and the spiders: data only

None of `AbstractSkeleton`, `Skeleton`, `Stray`, `Bogged`, `Parched`, `Creeper`, `Spider`,
`CaveSpider` is dumped (section 9). What the extraction gives: their sizes and attributes
(3.0), tags (skeleton, stray and bogged burn; parched does not), loot tables
(`entities/skeleton`, `stray`, `bogged`, `creeper`, `spider`, `cave_spider`), and that the
bogged is shearable in the game (`shearing/bogged` is a table of the jar that Clustine has
not extracted). Their equipment (the bow), synched data (the creeper's swell, charge and
ignition; the spider's climbing flags), conversion (a skeleton freezing into a stray) and
`finalizeSpawn` (the spider's jockey and effects) are not read.

### 3.6 `Witch` (Witch.txt, 702 lines)

`extends Raider implements RangedAttackMob`.

- **Synched** 17 `DATA_USING_ITEM` boolean (122–134; Raider's 16 celebrating). Not saved
  beyond `Raider`'s.
- **Goals** (23–121): 1 `FloatGoal`; 2 `RangedAttackGoal(1.0, 60, 10.0f)`; 2
  `WaterAvoidingRandomStrollGoal(1.0)`; 3 `LookAtPlayerGoal(Player, 8)`; 3
  `RandomLookAroundGoal`; targets 1 `HurtByTargetGoal(Raider)`, 2
  `NearestHealableRaiderTargetGoal(Raider, …)`, 3 `NearestAttackableWitchTargetGoal(Player,
  10, true, false)`.
- **`aiStep`** (181–406), on the server while alive:
  1. the healing goal's cooldown decremented; the player goal allowed when it is 0;
  2. if drinking: `usingTime` down; at 0: not using, the main hand emptied, each effect of
     the potion applied (scaled by its duration component), game event `DRINK`, the
     drinking speed modifier removed;
  3. else the choice, each `r.nextFloat()` drawn in turn until one passes: `< 0.15` and the
     eye in water and no water breathing → water breathing; `< 0.15` and (on fire, or last
     hurt by fire) and no fire resistance → fire resistance; `< 0.05` and health below the
     maximum → healing; `< 0.5` and a target farther than 11 without speed → swiftness;
     with a potion: in the main hand, `usingTime` its use duration, using on,
     `witch_drink` at pitch `0.8 + nextFloat() × 0.4`, the modifier `drinking` −0.25
     `ADD_VALUE` on speed;
  4. `r.nextFloat() < 7.5E-4f` → entity event 15 (the client's 10 to 44 purple particles,
     412–468);
  5. `Raider.aiStep`.
  All draws are the own random's. A hurt witch in N7 drinks healing about once in 28
  ticks (`0.85 × 0.85 × 0.05`); whether the potion heals needs mob effects (healing is instant, **Inferred** 4
  health for an unamplified potion).
- `getDamageAfterMagicAbsorb` (469–492): magic hurts it less (**Inferred** the 0.15 factor);
  `canBeLeader` false (666–669); sounds `witch_ambient`, `witch_hurt`, `witch_death`,
  `witch_celebrate`. `performRangedAttack` (493–656) throws potions (N9).

### 3.7 The illagers

- **`Pillager`** (512 lines): synched 17 `IS_CHARGING_CROSSBOW` (164–176); saved the
  inventory (220–263; read sets `canPickUpLoot` true); goals 0 `FloatGoal`, 1
  `AvoidEntityGoal(Creaking, 8, 1.2, …)`, 2 `HoldGroundAttackGoal(10)`, 3
  `RangedCrossbowAttackGoal(1.0, 8)`, 8 `RandomStrollGoal(0.6)`, 9 `LookAtPlayerGoal(Player,
  15)`, 10 `LookAtPlayerGoal(Mob, 15)`; targets `HurtByTargetGoal(Raider)`, players,
  villagers, iron golems. `finalizeSpawn` (274–295): equipment first (a crossbow, 296–306),
  its enchantment (`enchantSpawnedWeapon`, 307–333: Mob's 0.25 × s draw, then
  `r.nextInt(300) == 0` with a crossbow → the provider `pillager_spawn_crossbow`, Piercing
  I), then `AbstractIllager`'s, which is `Raider`'s. Sounds `pillager_*`. Arm pose by
  crossbow state (230–250).
- **`Vindicator`** (405 lines): saved `Johnny` when true (208–249); goals 0 `FloatGoal`, 1
  avoid the creaking, 2 `VindicatorBreakDoorGoal`, 3 `RaiderOpenDoorGoal`, 4
  `HoldGroundAttackGoal(10)`, 5 `MeleeAttackGoal(1.0, false)`, 8 `RandomStrollGoal(0.6)`, 9
  `LookAtPlayerGoal(Player, 3)`, 10 `LookAtPlayerGoal(Mob, 8)`; `customServerAiStep`
  (169–190) opens doors in a raided village; `finalizeSpawn` (255–282): `Raider`'s first,
  then the navigation opens doors, then an iron axe outside a raid (283–296) and its
  enchantment draw (0.25 × s); a name `Johnny` makes it hostile to all (297–316). Arm pose
  attacking, celebrating or crossed. Sounds `vindicator_*`.
- **`Evoker`** (288 lines) `extends SpellcasterIllager`: synched 17
  `DATA_SPELL_CASTING_ID` byte (SpellcasterIllager.txt 25–37); saved `SpellTicks` (38–62);
  goals 0 `FloatGoal`, 1 casting, 2 `AvoidEntityGoal(Player, 8, 0.6, 1.0)`, 3 avoid the
  creaking, 4 summon, 5 fangs, 6 the wololo (a blue sheep turned red), 8
  `RandomStrollGoal(0.6)`, 9 `LookAtPlayerGoal(Player, 3)`, 10 `LookAtPlayerGoal(Mob, 8)`;
  no `finalizeSpawn` of its own (`Raider`'s). Sounds `evoker_*`; casting sounds by spell.

### 3.8 The guardians

- **`Guardian`** (924 lines): experience 10, water malus 0, `GuardianMoveControl`, a
  **construction draw** `random.nextFloat()` for the client's tail (27–56);
  `WaterBoundPathNavigation` (164–172); synched 16 `DATA_ID_MOVING` boolean, 17
  `DATA_ID_ATTACK_TARGET` int (the beam's target id, 0 none) (173–191); goals 4
  `GuardianAttackGoal`, 5 `MoveTowardsRestrictionGoal(1.0)`, 7 `RandomStrollGoal(1.0, 80)`,
  8 `LookAtPlayerGoal(Player, 8)`, 8 `LookAtPlayerGoal(Guardian, 12, 0.01)`, 9
  `RandomLookAroundGoal`; target `NearestAttackableTargetGoal(LivingEntity, 10, true,
  false, GuardianAttackSelector)` (57–149). Ambient interval 160; sounds by whether it is in
  water (`guardian_ambient` or `_land`, …); `getMovementEmission` events only.
- **`aiStep`** (363–743), on the server: in water, air 300; else on the ground, the flop:
  motion += `((nextFloat() × 2 − 1) × 0.4, 0.5, (nextFloat() × 2 − 1) × 0.4)` (floats
  widened), yaw `nextFloat() × 360`, off the ground, `needsSync`; with a beam target the
  body turned to the head; then `Monster.aiStep`. On land without water it suffocates by the
  body's air rule (**Inferred**: it cannot breathe air).
- **`hurtServer`** (831–874): not moving, a source not of `#avoids_guardian_thorns` nor
  thorns, a living direct entity: that entity hurt by `thorns` 2; then the stroll triggered
  and `Monster.hurtServer`.
- **`ElderGuardian`** (155 lines): persistent from construction, stroll interval 400;
  attack duration 60; sounds `elder_guardian_*` (`_land` out of water), flop
  `elder_guardian_flop`; `customServerAiStep`: every 1200 ticks by `(tickCount + id) % 1200`,
  mining fatigue III for 6000 ticks to **survival** players within 50
  (`MobEffectUtil.addEffectToPlayersAround` with the display limit 1200: **Inferred** those
  who have less than that left), with the game event packet `GUARDIAN_ELDER_EFFECT` (silent: 0) to each; home
  set at its position, radius 16, if it has none.

### 3.9 What each does in N7 with no goals, and with the plan's reduced ones

P 11.2 makes monsters "bodies that float, stroll and look". With no goals at all a body
stands, falls, is pushed and sinks (no `FloatGoal`); the reduced set takes each type's own
float, stroll and look goals and leaves its targets and attacks to N9.

| Type | Float, stroll, look goals of its own | With none | Always, goals or not |
|---|---|---|---|
| zombie, husk, zombie villager | no float; 7 `WaterAvoidingRandomStrollGoal(1.0)`, 8 `LookAtPlayerGoal(Player, 8)`, 8 `RandomLookAroundGoal` | sinks, walks the bottom | burns (not the husk); in water converts (zombie 45 s to a drowned, husk to a zombie) |
| drowned, skeleton, stray, bogged, creeper, spider, cave spider | not dumped | — | the first four burn |
| witch | 1 float, 2 water-avoiding stroll, 3 look, 3 look around | sinks | the potion draws; drinks when hurt or on fire |
| pillager, vindicator, evoker | 0 float, 8 `RandomStrollGoal(0.6)`, 9 and 10 looks | sinks | — |
| guardian, elder guardian | 5 restriction, 7 stroll (80; 400 for the elder), 8 looks, 9 look around | drifts | flops on land; spikes; the elder's curse (survival only) |

The plan's "float" is a goal only some have: a zombie in the game does not float, which is
why it walks under water and converts. Giving every monster a float would change that.

---------------------------------------------------------------------------------------

## 4. Spawn eggs, and a player's use of each

Every type of this brief has an egg in 26.3 (`items.rs`): iron golem, snow golem,
villager, wandering trader, zombie, husk, drowned, zombie villager, skeleton, stray,
bogged, parched, creeper, spider, cave spider, witch, pillager, vindicator, evoker,
guardian, elder guardian. Today `spawn_eggs.rs` (`use_spawn_egg_on` 69, `use_spawn_egg`
97) makes nothing for a type not built. An egg's mob is made by `EntityType.spawn` with
`SPAWN_ITEM_USE` and finalised (B 1.11.1–1.11.3), drawing as section 5 says; on water the
same. `EntityType.canSpawn` refuses a monster's egg in peaceful (B 1.11.1), which Clustine
never is (section 10).

**An egg on a living mob of its own type** (B 1.11.4): the zombie family makes a baby
(the zombie's `setBaby` works), and a zombie's egg baby gets the loot draw of 3.4.1; a
villager makes a baby by 2.3's rule; the wandering trader makes nothing; every other type
of this brief makes nothing (`Mob.setBaby` does nothing, the child is refused, its
construction draws made and lost: a zombie villager's profession, a guardian's tail).

**A player's use, by type** (after the name tag, the egg and the leash of `Mob.interact`,
B 1.10.2):

| Type | Use | What happens |
|---|---|---|
| iron golem | an iron ingot | heals 25, the repair sound with two own draws; at full health nothing (`PASS`) (1.2) |
| snow golem | shears | the pumpkin off (a carved pumpkin dropped at the eye), `snow_golem_shear`, game event `shear`, the shears worn by 1 (1.4) |
| villager | anything but its egg | a baby: unhappy and `villager_no`; an adult with no offers: the same with the main hand, nothing with the off hand; offers: the merchant screen (2.5) |
| wandering trader | anything but a villager egg | the merchant screen with its offers (2.7) |
| zombie villager | a golden apple | only with weakness (none in N7): nothing (3.4.3) |
| any of them, lead in hand | — | the leash, B 1.10.4: `Mob.canBeLeashed` (Mob.txt 3078–) is true unless the mob is an `Enemy`, so golems take a lead and no monster does; `AbstractVillager.canBeLeashed` (351–355) is false: villagers and wandering traders take none |
| the rest | anything | `PASS`, no event |

Not used by a player but by a dispenser: shears on a snow golem (`ShearsDispenseItemBehavior`,
n6, any `Shearable`), a carved pumpkin placed only where a golem can be built
(`canSpawnGolem`, 1.5). Clustine's dispensers have neither.

---------------------------------------------------------------------------------------

## 5. The draws of each `finalizeSpawn`, in order

R is the random the caller hands `finalizeSpawn` (`level.getRandom()`: for an egg, the
player's stream aside in Clustine, `spawn_eggs.rs`; for a structure, the run's region
random, B 10.8). O is the mob's own random. `s` is the special multiplier. "Tail" is the
Mob tail (Mob.txt 2645–2683): `nextDouble()`, `nextDouble()` for the follow-range
triangle unless the `random_spawn_bonus` modifier is there already, then `nextFloat()` for
the left hand (< 0.05). "Leader" is `PatrollingMonster`'s: unless `PATROL`, `EVENT` or
`STRUCTURE`, `nextFloat()` (< 0.06 and able to lead: the banner). "Armour" is 3.2's first
part: `nextFloat()` (< 0.15 s), and if it passes `nextInt(3)`, three `nextFloat()`, then a
`nextFloat()` before each slot after the first. "Ench(slots)" is a `nextFloat()` for each
listed slot that holds something (main hand < 0.25 s, armour < 0.5 s), followed by the
provider's draws (not dumped) when it passes.

| Type | `SPAWN_ITEM_USE` (egg) | `STRUCTURE` | `NATURAL` (N9) | Notes |
|---|---|---|---|---|
| iron golem, snow golem | Tail | Tail (the village and outpost golems' templates carry the bonus: the hand only, B 8.12) | — | built from a pumpkin: none (`TRIGGERED`, no `finalizeSpawn`) |
| villager | Tail | Tail | — | no baby draw (group `false`); type by biome, no draw |
| wandering trader | Tail | — | Tail (its spawner, N9) | |
| zombie | Z | — (no template zombie) | Z | Z below |
| husk | Z, then R `nextFloat()` (loot again) | — | Z, loot, then if room R `nextFloat()` (< 0.1: the camel husk and the parched, each finalised from R) | |
| zombie villager | (O: profession at construction) Z | Z (zombie villages, igloo not finalised; B 8.12) | Z | `finalizeVillagerType` first, no draw |
| drowned, skeleton, stray, bogged, parched, creeper, spider, cave spider | not dumped | not dumped | not dumped | |
| witch | Leader, Tail | Tail (swamp hut, B 8.13) | Leader, Tail | the witch cannot lead: the 0.06 is drawn and does nothing |
| pillager | Ench(main hand), R `nextInt(300)`, Leader, Tail | — (outposts' pillagers are N9's patrols and spawns) | the same | the crossbow set first, no draw |
| vindicator | Leader, Tail, Ench(main hand) | Tail, Ench(main hand) (mansion, B 8.13) | the same as the egg | the axe set without a draw outside a raid |
| evoker | Leader, Tail | Tail (mansion) | — | |
| guardian | (O: tail at construction) Tail | — | Tail | |
| elder guardian | (O) Tail | (O) Tail (monument, B 8.13) | — | |

**Z**, `Zombie.finalizeSpawn` (Zombie.txt 946–1140) and `handleAttributes` (1202–1300):

1. Tail (from `Monster`'s, the super call, first).
2. Unless `CONVERSION`: R `nextFloat()` (< 0.55 s: can pick up loot).
3. The group, if null: R `nextFloat()` (< 0.05: a baby), can ride a jockey.
4. A baby: `setBaby(true)`; then R `nextFloat()` (as a double, < 0.05: ride the first
   chicken within 5, 3, 5 that carries nobody, if any); else R `nextFloat()` (< 0.05: a
   chicken made with `JOCKEY`, snapped to the zombie's position and yaw, finalised from R
   with its own draws (variant, sound and tail, B 10.8), made a jockey, ridden, added).
5. R `nextFloat()` (< 0.1 s: can break doors).
6. Unless `CONVERSION`: Armour; R `nextFloat()` (< 0.01, 0.05 on hard: a weapon) and then
   R `nextInt(6)`; Ench(main hand, then the armour slots in `EquipmentSlot.VALUES` order:
   feet, legs, chest, head, **Inferred** from the enum).
7. On 31 October (`SpecialDates.isHalloween`, the server's clock) with an empty head: R
   `nextFloat()` (< 0.25: a pumpkin) and then R `nextFloat()` (< 0.1 a jack o'lantern, else
   a carved pumpkin), head drop chance 0.
8. O `nextDouble()` (reinforcement chance), O `nextDouble()` (knockback bonus), O
   `nextDouble()` (follow range), O `nextFloat()` (leader, < 0.05 s) and for a leader O
   `nextDouble()` twice.

At `s = 0` (easy, or Clustine today) a zombie from an egg draws: Tail (3), loot (1), baby
(1), [a baby: 1 or 2, a new chicken's own], doors (1), armour (1), weapon (1, and `nextInt(6)`
one time in a hundred), Ench for a held weapon (1), [Halloween 1–2], then O 4. **Inferred**:
`EquipmentSlot.VALUES` is the enum's order (main hand, off hand, feet, legs, chest, head,
body, saddle), Clustine's `living.rs` 33.

---------------------------------------------------------------------------------------

## 6. Synched data and what clients are sent

| Type | Index: accessor (serializer) | Also |
|---|---|---|
| iron golem | 16: flags (`BYTE`), mask 1 player-created | entity events 4 (attack), 11 and 34 (flower); sounds attack, damage, repair, hurt, death, step |
| snow golem | 16: pumpkin (`BYTE`), mask 0x10, default 0x10 | sounds shoot, shear, ambient, hurt, death; snow placed (block updates) |
| villager | 16 baby, 17 age locked (AgeableMob); 18 unhappy (`INT`); 19 `VillagerData` (`VILLAGER_DATA` 18); 20 finalised (`BOOLEAN`) | `villager_no`, ambient; entity events 12, 13, 14, 42 not in N7 |
| wandering trader | 16, 17; 18 unhappy | |
| zombie, husk | 16 baby, 17 special type (`INT`), 18 drowned conversion (`BOOLEAN`) | Mob's 15 flags: left-handed, aggressive (N9) |
| zombie villager | 16–18; 19 converting; 20 `VillagerData`; 21 finalised | the profession shows on its skin |
| witch | 16 celebrating (Raider); 17 using item (`BOOLEAN`) | the potion in its main hand (`SetEquipment`); entity event 15; `witch_drink` |
| pillager | 16 celebrating; 17 charging crossbow | the crossbow in hand; the leader's banner on its head |
| vindicator | 16 celebrating | the axe in hand |
| evoker | 16 celebrating; 17 spell (`BYTE`) | |
| guardian, elder guardian | 16 moving (`BOOLEAN`); 17 attack target (`INT`) | the elder's game event packet to survival players |
| every monster | Mob's and LivingEntity's (B 1.15): health (the iron golem's cracks), the fire flag of shared flags (burning), equipment (armour, weapons, pumpkins) | |

What the edge lacks for these: a `VILLAGER_DATA` value in `DataShown` and its writer
(`clustine-sim/src/shown.rs` 16–81; `services/edge/src/tracking.rs` 340–369), with the
registry ids of villager types and professions (the protocol's `registries.json` order;
they are not synchronised registries, **Inferred** their ids are the built-in registry's).
Item stacks in hand are sent by `SetEquipment` already (`region/mob_bodies.rs` 383–426).

---------------------------------------------------------------------------------------

## 7. What crosses a line (B section 11's manner)

- **A golem's attack target across a line.** The iron golem's target search (follow range
  16, every fifth tick) and `MoveTowardsTargetGoal`'s 32 read the copies (ADR-0028 section
  3), a tick old; the target is a key. Its hit across a line is an entity effect (ADR-0028
  section 7, as B 11.1's hurt) that must carry the **fling** with it: the holder applies
  `hurtServer` and, **only if it hurt**, adds `(0, 0.4 × max(0, 1 − kr), 0)` with the
  target's own knockback resistance, which only the holder knows. The sender cannot know
  whether it hurt; the attack sound is played at the golem either way. The attack animation
  (entity event 4) is the golem's own region's.
- **The golem's push** (B 11.8): the push is the body's at a line; the
  `nextInt(20)` that targets the pushed enemy is drawn in the golem's tick by the golem's
  region, the target a key across the line.
- **The flower.** The candidate is read from copies within 6, 2, 6; a villager across the
  line is only looked at; a copper golem across the line gets the poppy as an effect (P 11.4
  names "the copper golem's flower").
- **The snow trail at a line.** A snow golem within 0.25 of a line places snow in another
  region's land: a `SetIf` block effect (ADR-0025) whose condition is the game's (air, and
  a snow layer can survive on the block below, read by the holder), and the `block_place`
  game event at that cell (sculk sensors across the line: whatever Clustine does for game
  events at a line today).
- **The snowball** across a line is N9's projectile.
- **A villager's data.** `VillagerData` is in the type's bytes (`kind`) and in its `Shown`;
  copies carry it, so a typed read across a line (an iron golem's gift candidates by tag
  need only the type; a zombie's turning of a villager is N9's) has it. The brain's
  memories are the mind and not in copies (P 11.2): a golem's "villager who wants a golem"
  across a line is "unknown", which is false in N7 everywhere anyway.
- **Burning** reads only the mob's own column: its eye position and the sky above it, in
  its own region's land (a mob is held by the region of its chunk). The light at a line is
  the stored light either side (P 11.6).
- **A zombie's chicken at spawn.** The search for a chicken within 5, 3, 5 reads copies;
  a found chicken across the line is ridden: a mount across a line, which N7j's
  `n7j-mount-across-a-line.md` treats. A new chicken is made in the zombie's region, at its
  position.
- **The elder guardian's curse** reaches survival players within 50 wherever they are held:
  N9's, with mob effects and survival.
- **The guardian's spikes** hurt the attacker: a player across a line hitting it (the
  reach is 3) would take a hurt effect back; a creative player is not hurt.
- **Raids, patrols, gossip** are N10's and N9's.

---------------------------------------------------------------------------------------

## 8. What Clustine has to reuse, and what is missing

Read in `crates/clustine-sim/src/mobs/` and around (paths below are of `clustine-sim/src/`
unless whole).

**There**:

- The body (`mobs/body.rs` 269: vitals, attributes, equipment with drop chances at 0.085,
  flags with persistence, `NoAI`, left-handed, `picks_up_loot`), hurting and death
  (`body/hurt.rs`: armour 383, knockback 403, death drops 547), entity loot tables
  (`body/loot.rs`, no enchantments; every table of this brief's types is extracted:
  `clustine-data/src/generated/loot_tables.rs` 1849–1949).
- The tick's order and outputs (`mobs/tick.rs`: `MobTick` 187, `Local::Hurt` 105 between
  mobs, sounds, level and game events, `SetBlock`), the read view (`mobs/view.rs`: light
  142, sky darkening 113, the clock's day 120, game time 97).
- The goals: `FloatGoal`, `PanicGoal`, `RandomStrollGoal`, `RandomLookAroundGoal`,
  `LookAtPlayerGoal`, `TemptGoal`, `AvoidEntityGoal`, `MoveToBlockGoal`,
  `MoveTowardsRestrictionGoal`, `HurtByTargetGoal`, `NearestAttackableTargetGoal`,
  `MeleeAttackGoal` with `melee_hit`, `UseItemGoal` (`mobs/ai/goals.rs` 131–1714); a
  `RangedAttackGoal` in `types/llama.rs` 532 whose attack is a sound; anger
  (`types/neutral.rs`), with `ResetUniversalAngerTargetGoal` a no-op as universal anger is
  off (`types/wolf.rs` 683).
- The brain's engine and every behaviour B 7.14.2 needs for a reduced villager
  (`mobs/brain/behaviours/`: `Swim` in `panic.rs`, `InteractWithDoor` in `doors.rs`,
  `LookAtTargetSink`, `MoveToTargetSink`, `RandomStroll`, `SetWalkTargetFromLookTarget` in
  `walking.rs`, `SetEntityLookTarget` in `looking.rs`; `RunOne` gates; the 26 activities,
  `brain/activity.rs` 13), the sensors `NEAREST_LIVING_ENTITIES` and `NEAREST_PLAYERS`
  (`brain/sensors.rs` 185, 211), an `InventoryCarrier`'s pick-up (`types/allay.rs` 707).
- The type dispatch (`mobs/types.rs` 1069 `built`), the shared maker (`mobs/making.rs`:
  `construct` 20, `finalize` 40 with the Mob tail), `Spawning` with a `difficulty` field
  (`types.rs` 907–959, unused), the generated maker (`generation.rs` 79).
- Fire on entities (`living.rs` 200–456; set alight by blocks in `living/inside.rs`
  139–264), shown by the shared flag (`region/mob_bodies.rs` 637; the edge's
  `tracking.rs` 54).
- Building a golem: the copper golem's hand-written check on `onPlace`
  (`region/copper_golems.rs` 84, called from `region/updates.rs` 254–256), making the mob
  with `new_mob` and `add_made_mob` (`region/animals.rs` 383, 445), clearing with level
  event 2001.
- Shearing a mob (`types/sheep.rs` 315: sound, `named_drops`, `Did::Drop`, game event).
- `Family::Kept` for golems and villagers that never despawn (`body/ladder.rs` 18, 71–77).

**Missing** (each with where N7k needs it):

| Missing | For |
|---|---|
| Modules for the 21 types | everything; zombie horse and zombie nautilus exist and lack burning |
| A `Monster` family (sound source hostile, `noActionTime` +2, walk value, the bed rule) | 3.1; `body/ladder.rs` puts monsters in `Other` |
| `burnUndead` with `monsters_burn`, the magic light value, `canSeeSky`, the helmet's wear | 3.3 |
| `populateDefaultEquipmentSlots`, the enchantment draws, any enchantment (no registry, no item components) | 3.2, 3.4, 3.7 |
| The difficulty: a world's, `DifficultyInstance`'s effective value and special multiplier | section 5; every caller passes `(0, 0)` |
| `VillagerData`, villager types by biome, professions' registry ids; the edge's writer | 2.1, section 6 |
| Golem goals: `MoveTowardsTargetGoal`, `MoveBackToVillageGoal`, `GolemRandomStrollInVillageGoal`, `OfferFlowerGoal`, `DefendVillageTargetGoal`; a type's `doHurtTarget` | 1.3 |
| Villager sensors and behaviours beyond B 7.14.2 (`VillagerPanicTrigger`, the rest package) | 2.4, N10 |
| Projectiles (snowball, arrow, potion, crossbow bolt) | snow golem; skeletons, witch, pillager in N9 |
| Mob effects | husk's hunger, the cure, the witch's potions, the elder's curse, the trader's invisibility |
| `ConversionTracker` (the drowning conversion), conversions to another type of mob | 3.4 (Clustine has `Did::Convert` for a player's use only, `types.rs` 872) |
| Environment attributes as data (`monsters_burn`, `snow_golem_melts`) | hand-written lists, as `bees_stay_in_hive` and `increased_fire_burnout` are |
| Game rules (`mob_griefing`, `mob_drops`) | taken as on |
| `shearing/snow_golem` and `shearing/bogged` tables | 1.4, 3.5 |
| Trading: offers, trade sets, the merchant menu | 2.5, 2.7 (N10) |
| Rain and weather | wetting (no burning in rain), the snow golem's hurt |

---------------------------------------------------------------------------------------

## 9. Classes not dumped

Of the classes this section first listed, all but one were dumped on 2026-10-21 into
`n7k-javap/` and are read in section 11: the drowned and its inner classes, the skeletons
(`net.minecraft.world.entity.monster.skeleton`), the creeper, the spiders
(`net.minecraft.world.entity.monster.spider`), `ConversionTracker`, `Crackiness`,
`BlockPattern` and its parts, `Projectile`, `ThrowableProjectile`,
`ThrowableItemProjectile`, `Snowball`, the enchantment providers, the zombie's and husk's
group data and the turtle egg goal, `MerchantOffers`, `TradeSet`, `TradeSets`,
`GossipContainer`, `Raid`, `Raids`, `MobEffects`, `Potions`, `RangedAttackMob`,
`RangedCrossbowAttackGoal`, `RestrictSunGoal`, `FleeSunGoal`, `RangedBowAttackGoal`,
`SwellGoal`, `LeapAtTargetGoal`. Still not in any dump, and named where section 11 needs
them:

- `net.minecraft.world.level.BlockAndTintGetter` (`canSeeSky`, `getRawBrightness`: 3.3's
  burning and 11.2's sun goals; **Inferred** as before)
- `net.minecraft.world.entity.projectile.ProjectileUtil` (`getHitResultOnMoveVector`,
  `getMobArrow`, `getWeaponHoldingHand`: 11.2, 11.10)
- `net.minecraft.world.entity.projectile.arrow.AbstractArrow`, `Arrow`, `ThrownTrident`
  (the skeletons' and the drowned's shots: N9)
- `net.minecraft.world.entity.AreaEffectCloud` (a creeper's lingering cloud: never in N7k)
- `net.minecraft.world.effect.InstantenousMobEffect` and the other effect classes (what a
  potion does, 3.6, 11.11)
- `net.minecraft.world.item.enchantment.Enchantment` (costs, levels, primary items; the
  providers' selection, 11.9; its rules are the registry's data)
- `net.minecraft.world.level.block.state.predicate.BlockStatePredicate` (the golem patterns'
  `#` and its test of a missing state, 11.7)
- the target of `CarvedPumpkinBlock`'s `invokedynamic` #2, the iron golem's `~` (11.7): a
  method reference, which `javap -c` does not resolve; `javap -v` of `CarvedPumpkinBlock`
  (its bootstrap methods) would show it

---------------------------------------------------------------------------------------

## 10. Open questions for the plan

1. **Which difficulty does Clustine run at?** None exists; eggs pass `(0, 0)` and
   generation takes easy (L 3.6). Easy keeps every special draw at 0 and leaves the zombie's
   weapon at 0.01; `level.getDifficulty() == HARD` tests (weapons, armour's stop chance,
   reinforcements, doors) read the world's. A world setting with easy as its default, sent
   to clients by `change_difficulty`, would be the game's.
2. **The snow golem's snowball**: a sound and nothing thrown (named) until N9's
   projectiles, or a reduced snowball now? A player who builds one near a zombie sees it
   aim and throw nothing; the knockback a snowball gives is what keeps zombies off.
3. **The iron golem fighting monsters**: in N7k (its target goals and `doHurtTarget`, the
   hurt and fling across a line), or with N9's attacks? With monsters' bodies in villages
   (zombie villages) it is ordinary play.
4. **The villager's brain in N7k**: none, B 7.14.2's reduction, or that with a night rest
   (P 11.7 question 5)? The rest package without beds is not read (2.4). A villager with no
   brain sinks.
5. **The wandering trader's screen**: the game always opens it with offers. Refuse (`PASS`,
   or `CONSUME` as for an empty villager), or open it empty, until N10's trades?
6. **Floating monsters**: P's "bodies that float, stroll and look" gives zombies a float
   the game does not; the zombie's drowning conversion depends on it sinking. Take each
   type's own goals (3.9)?
7. **The drowning conversion**: in N7k (a zombie in water becomes a drowned in 45 seconds,
   which a player sees) needs the drowned, which is not dumped; or named until N9?
8. **The witch's potions** without mob effects: drink (the item, sound and slowness) and
   apply nothing, or not drink at all? The draws are made either way.
9. **The six types not dumped** (drowned, skeleton family, creeper, spiders): a fourth dump
   before the plan, or N7k without them (their eggs then make nothing, and ocean ruins'
   drowned, L 1.2, wait)? **Answered: dumped** on 2026-10-21 (`n7k-javap/`) and read in
   section 11; the plan takes them into N7k (its question 9). Questions 7 and 10 lose
   "not dumped" with it (11.1, 11.5, 11.7).
10. **Golems built lying down**: the hand-written upright shapes, named as the copper
    golem's are, or `BlockPattern` dumped and ported?
11. **Burning for the zombie horse and zombie nautilus** comes with N7k's `burnUndead`:
    confirm it is wanted now (it is the game's, and visible).

---------------------------------------------------------------------------------------

## 11. The classes dumped on 2026-10-21

59 classes, `javap -p -c -constants`, in `/home/claude/reference/n7k-javap/` (`classes.txt`
lists them), one file per class named by its full class name; "Drowned.txt 194–294" below
is `net.minecraft.world.entity.monster.zombie.Drowned`'s, and a file of an earlier dump says
so ("n7 Mob.txt"). This section supersedes what 3.4.4, 3.5 and 3.9's rows for these types
and section 5's "not dumped" row say. The conventions of the preamble hold (`double` is
`f64`, `float` is `f32`; a `float` compared with a `double` constant is widened first).
"Tail", "Armour", "Ench(…)", "Z", R and O are section 5's.

### 11.1 The drowned (Drowned.txt, 845 lines, and its six inner classes)

`Drowned extends Zombie implements RangedAttackMob`.

- **Construction** (13–32): `Zombie`'s (no draw, n7 Zombie.txt 45–80, its drowning tracker
  built but never ticked, below), then `rangedAttackUncertainty = −1.0f` (meaning "the
  interface's", 749–762), `moveControl = DrownedMoveControl`, path malus `WATER` 0.
  `createNavigation` (42–49) is **`AmphibiousPathNavigation`** (Clustine has the kind,
  `mobs/ai/navigation.rs` 26). `createAttributes` (34–40) is the zombie's with step height 1.
  No draw.
- **Synched**: none of its own (no `defineSynchedData`): the zombie's 16 baby, 17 special
  type, 18 drowned conversion (n7 Zombie.txt 251–275). **Saved**: the zombie's, *including*
  the tracker's `InWaterTime` and `DrownedConversionTime`, written unconditionally (n7
  Zombie.txt 815–858); the drowned never ticks them (`convertsInWater` false, 496–499).
  `searchingForLand` and the uncertainty are not saved.
- **Baby** dimensions (static 829–844): `scalable(0.49, 0.98)`, eye 0.775, vehicle
  attachment (0, 0.1875, 0), as the zombie's.
- **Goals.** `Zombie.registerGoals` (n7 Zombie.txt 89–122) adds 4
  `ZombieAttackTurtleEggGoal(1.0, 3)` (11.8), 8 `LookAtPlayerGoal(Player, 8.0f)`, 8
  `RandomLookAroundGoal`, then calls `addBehaviourGoals`, which the drowned replaces (51–192):

  | Priority | Goal | Without a target |
  |---|---|---|
  | 1 | `DrownedGoToWaterGoal(1.0)` | by day, on land: walks to water (below) |
  | 2 | `DrownedTridentAttackGoal(1.0, 40, 10.0f)` | `RangedAttackGoal`'s `canUse` needs a target |
  | 2 | `DrownedAttackGoal(1.0, false)` | `ZombieAttackGoal`'s needs a target, and `okTarget` |
  | 5 | `DrownedGoToBeachGoal(1.0)` | at night, in water near the surface: walks to land |
  | 6 | `DrownedSwimUpGoal(1.0, seaLevel)` | at night, in water, deep: swims up |
  | 7 | `RandomStrollGoal(1.0)` | strolls (not water-avoiding), on land or the bottom |
  | targets 1 | `HurtByTargetGoal(Drowned)`, alerting `ZombifiedPiglin` | N9 |
  | targets 2, 3, 3, 3, 5 | players (`okTarget`), `AbstractVillager`, `IronGolem`, `Axolotl`, `Turtle` babies on land | N9 |

  `okTarget(t)` (508–524): `t != null && (!level.isBrightOutside() || t.isInWater())`. The
  zombie's `SpearUseGoal`, `ZombieAttackGoal`, `MoveThroughVillageGoal` and
  `WaterAvoidingRandomStrollGoal` are **not** the drowned's.
- **`DrownedGoToWaterGoal`** (Drowned$DrownedGoToWaterGoal.txt): flags `MOVE` (30–32).
  `canUse` (35–69): `level.isBrightOutside()`, not `isInWater()`, then `getWaterPos()`
  non-null. `getWaterPos` (100–146), with the mob's **own** random, up to ten tries, each
  `p = blockPosition().offset(nextInt(20) − 10, 2 − nextInt(8), nextInt(20) − 10)` (three
  draws, x then y then z) and stop at the first whose block **is `Blocks.WATER`** (a
  waterlogged block is not), returning its bottom centre. So a drowned on land by day
  draws 3 to 30 times **every time the goal is asked**. `start` (83–98): `moveTo(x, y, z,
  1.0)`; continues while the path is not done.
- **`DrownedGoToBeachGoal`** (Drowned$DrownedGoToBeachGoal.txt): a `MoveToBlockGoal(drowned,
  1.0, 8, 2)` (5–16). `canUse` (18–47) asks **`MoveToBlockGoal.canUse` first** (its
  countdown and, at 0, the own `nextInt(200)` and the search, n7 MoveToBlockGoal.txt 68–100),
  by day too, then `!isBrightOutside()`, `isInWater()`, `getY() >= seaLevel − 3`. A target
  cell (55–79): the two cells above empty (`isEmptyBlock`) and the cell's state
  `entityCanStandOn`. `start` (81–89) clears `searchingForLand`, then `MoveToBlockGoal`'s.
- **`DrownedSwimUpGoal`** (Drowned$DrownedSwimUpGoal.txt): **no flags** (the constructor,
  11–24, sets none: it runs beside any other goal). `canUse` (26–50): `!isBrightOutside()`,
  `isInWater()`, `getY() < seaLevel − 2`; continues while that holds and it is not `stuck`.
  `start` (128–137): `searchingForLand = true`, not stuck; `stop` (139–145): false. `tick`
  (65–126): if `getY() < seaLevel − 1` and (the path is done or `closeToNextPos()`, the path's
  target within 2, 623–655): `DefaultRandomPos.getPosTowards(this, 4, 8, (getX(), seaLevel − 1,
  getZ()), π/2 as f32 widened)` (n7 DefaultRandomPos.txt; own draws); none: `stuck`; else
  `moveTo(it, 1.0)`.
- **Swimming** (536–621): `wantsToSwim()` is `searchingForLand || (target != null &&
  target.isInWater())`. `updateSwimming` (588–608, server): swimming (the pose) =
  `isEffectiveAi() && isUnderWater() && wantsToSwim()`. `travelInWater` (556–586): under
  water and wanting to swim, `moveRelative(0.01f, input)`, `move(SELF, motion)`, motion ×
  0.9; else the zombie's (the body's water travel). `isPushedByFluid` (526–534): not while
  swimming. `isVisuallySwimming`: swimming and not a passenger.
- **`DrownedMoveControl.tick`** (Drowned$DrownedMoveControl.txt 10–210):
  ```
  t = drowned.getTarget()
  if drowned.wantsToSwim() && drowned.isInWater():
      if (t != null && t.getY() > drowned.getY()) || drowned.isSearchingForLand():
          motion += (0, 0.002, 0)
      if operation != MOVE_TO || navigation.isDone(): drowned.setSpeed(0); return
      dx, dy, dz = wanted − position (f64); d = sqrt(dx² + dy² + dz²); dy /= d
      yaw = (atan2(dz, dx) × 57.2957763671875) as f32 − 90
      setYRot(rotlerp(getYRot(), yaw, 90)); yBodyRot = getYRot()
      s = (speedModifier × MOVEMENT_SPEED) as f32
      speed = lerp(0.125f, getSpeed(), s); setSpeed(speed)
      motion += (speed × dx × 0.005, speed × dy × 0.1, speed × dz × 0.005)   // speed widened
  else:
      if !onGround(): motion += (0, −0.008, 0)
      MoveControl.tick()                                   // the walking control
  ```
  **So without a target a drowned swims only at night** (the swim-up goal sets
  `searchingForLand`): by day in water it is not swimming, sinks 0.008 a tick faster than the
  body's water travel would, and walks the bottom with the stroll (Inferred from the two
  paths above: nothing else makes it want to swim).
- **`finalizeSpawn`** (194–294): `Zombie.finalizeSpawn` first (section 5's Z, with the
  drowned's own `populateDefaultEquipmentSlots` in its step 6), then:
  ```
  if getItemBySlot(OFFHAND).isEmpty() && R.nextFloat() < 0.03f:
      OFFHAND = nautilus_shell; setGuaranteedDrop(OFFHAND)
  if (reason == NATURAL || reason == STRUCTURE) && main hand is a trident
          && R.nextFloat() < 0.5f && !isBaby()
          && !biome at blockPosition() in #more_frequent_drowned_spawns:   // = #is_river
      n = ZOMBIE_NAUTILUS.create(level, JOCKEY)
      if reason == STRUCTURE: n.setPersistenceRequired()
      n.snapTo(getX(), getY(), getZ(), getYRot(), 0)
      n.finalizeSpawn(level, difficulty, reason, null)     // its own draws, from R
      startRiding(n, false, false); level.addFreshEntity(n)
  ```
  The order of the `&&` is the draw order: the 0.5 is drawn for every adult or baby trident
  holder of those two reasons before `isBaby` and the biome are asked.
- **`populateDefaultEquipmentSlots(r, difficulty)`** (449–479), replacing the zombie's and
  **not calling `Monster`'s**: no armour draws and no zombie weapon:
  ```
  if (r.nextFloat() as f64) > 0.9:
      i = r.nextInt(16)
      MAINHAND = i < 10 ? trident : fishing_rod
  ```
  **Nothing here depends on the difficulty.** At special 0 (easy, Clustine today) a drowned
  from an egg holds a trident one time in 16 (0.1 × 10/16 = 0.0625) and a fishing rod 0.0375,
  carries a nautilus shell 0.03 (dropped surely), never armour; the held weapon's
  enchantment draw (`Ench(main hand)`, < 0.25 s) is made and never passes.
- **Picking up and keeping** (481–494, 808–820): `wantsToPickUp` refuses `#spears`, then the
  zombie's (refuses a glow ink sac, n7 Zombie.txt 932–945), then `Monster`'s;
  `canReplaceCurrentItem` keeps a nautilus shell. Only with `canPickUpLoot` (0.55 s: never at
  easy).
- **`performRangedAttack`** (657–747): a `ThrownTrident` of the held trident (or a new one),
  aimed as the snow golem's snowball with the target's `getY(1/3)`, `spawnProjectileUsingShoot`
  at 1.6 with `rangedAttackUncertainty` (the interface's 14 − 4 × difficulty id, 11.11);
  `drowned_shoot` at pitch `1 / (nextFloat() × 0.4 + 0.8)`, own random. N9's.
- **Sounds** (404–442): ambient `drowned_ambient_water` in water else `drowned_ambient`; hurt
  and death the same pairs; step `drowned_step` (at 0.15, 1, the zombie's `playStepSound`);
  swim `drowned_swim`; shoot above.
- **Riding** (789–806): `rideTick` turns its body with a `PathfinderMob` vehicle's (the
  nautilus).
- **Conversions it is the target of**: a zombie's drowning (`convertsToWhenDrowning`, n7
  Zombie.txt 737–740, and 11.5); none of its own. `checkSpawnObstruction` (501–506) is
  `isUnobstructed` only, `canSpawnInLiquids` true (444–447), and `checkDrownedSpawnRules`
  (296–376, N9's): water below or a spawner, dark enough unless the reason ignores light;
  rivers `nextInt(15) == 0`, elsewhere `nextInt(40) == 0` and below sea level − 5.

### 11.2 The skeletons

**`AbstractSkeleton`** (AbstractSkeleton.txt, 493 lines) `extends Monster implements
RangedAttackMob`.

- **Construction** (15–41): `bowGoal = RangedBowAttackGoal(this, 1.0, 20, 15.0f)`;
  `meleeGoal = AbstractSkeleton$1(this, 1.2, false)` (a `MeleeAttackGoal` that sets
  `aggressive` on start and clears it on stop, AbstractSkeleton$1.txt 20–38); then
  `reassessWeaponGoal()`. No draw. No synched or saved data of its own.
- **Goals** (`registerGoals`, 43–144): 2 `RestrictSunGoal`; 3 `FleeSunGoal(1.0)`; 3
  `AvoidEntityGoal(Wolf, 6.0f, 1.0, 1.2)`; 5 `WaterAvoidingRandomStrollGoal(1.0)`; 6
  `LookAtPlayerGoal(Player, 8.0f)`; 6 `RandomLookAroundGoal`; at 4 the bow or the melee goal
  (below); targets 1 `HurtByTargetGoal()`, 2 players, 3 iron golems, 3 turtle babies on land.
  **No `FloatGoal`**: a skeleton sinks, as a zombie does.
- **`reassessWeaponGoal`** (269–327), server only: remove both goals; the hand that holds a
  bow (`ProjectileUtil.getWeaponHoldingHand`, not dumped); a bow there: `bowGoal` at 4 with
  `setMinAttackInterval(hard ? getHardAttackInterval() : getAttackInterval())` (20 and 40;
  the bogged's and parched's 50 and 70); else `meleeGoal` at 4. Called by the constructor (an
  empty hand: melee), `finalizeSpawn`, `readAdditionalSaveData` (450–457) and every
  `onEquipItem` on the server (459–472). Neither goal runs without a target
  (RangedBowAttackGoal.txt 59–70; `MeleeAttackGoal`, n7).
- **`RestrictSunGoal`** (RestrictSunGoal.txt): no flags. `canUse` (14–34):
  `isBrightOutside()`, nothing on the head, ground navigation; `start` (36–51)
  `setAvoidSun(true)` on the `GroundPathNavigation`, `stop` (53–72) false. With it,
  `GroundPathNavigation.trimPath` (n7 GroundPathNavigation.txt, the `avoidSun` block before
  328) does, when the mob's own cell cannot see the sky, cut the path at its first node that
  can: a shaded skeleton by day does not walk into the sun. Clustine has the flag and names
  its trim (`mobs/ai/navigation.rs` 43).
- **`FleeSunGoal`** (FleeSunGoal.txt): flags `MOVE` (15–33). `canUse` (35–74): no target;
  `isBrightOutside()`; **on fire**; `canSeeSky(blockPosition())`; nothing on the head; then
  `getHidePos` (129–180), own random, up to ten tries of `blockPosition().offset(nextInt(20)
  − 10, nextInt(6) − 3, nextInt(20) − 10)` (x, y, z), the first that cannot see the sky and
  whose `getWalkTargetValue < 0` (`Monster`'s: the magic light value above 0.5, K 3.1): its
  bottom centre. `start` (112–127) `moveTo(…, 1.0)`; continues while the path is not done.
  So a burning skeleton in daylight draws 3 to 30 own values each time the goal is asked and
  walks into the nearest lit shade it finds. Never asked without burning (3.3).
- **`AvoidEntityGoal(Wolf)`**: a skeleton runs from any wolf within 6 (Clustine's
  `AvoidEntityGoal`, `mobs/ai/goals.rs` 666–), with or without a target: with wolves in the
  world this is ordinary play.
- **`populateDefaultEquipmentSlots`** (185–198): `Monster`'s (Armour, 3.2), then a bow in the
  main hand, no draw.
- **`finalizeSpawn`** (200–267), R the caller's:
  1. `Monster.finalizeSpawn` (the Tail);
  2. `populateDefaultEquipmentSlots(R)` (Armour; the bow);
  3. `populateDefaultEquipmentEnchantments(level, R)`: Ench(main hand, then the armour slots
     that hold something);
  4. `reassessWeaponGoal()` (the bow goal);
  5. `setCanPickUpLoot(R.nextFloat() < 0.55f × s)`;
  6. an empty head on Halloween: R `nextFloat()` (< 0.25), then R `nextFloat()` (< 0.1 a jack
     o'lantern, else a carved pumpkin), head drop chance 0.
- **Other**: `rideTick` (166–183) turns the body with a `PathfinderMob` vehicle (the spider);
  step sound at 0.15, 1 (154–162); `wantsToPickUp` refuses `#spears` (480–492);
  `isShaking` is `isFullyFrozen` (474–478); preferred weapons `#skeleton_preferred_weapons`
  (the bow). `performRangedAttack` (339–423): an arrow from `getArrow`
  (`ProjectileUtil.getMobArrow`, not dumped) aimed as the drowned's trident, 1.6 and the
  uncertainty; `skeleton_shoot` at `1 / (nextFloat() × 0.4 + 0.8)` (own): N9's.

**`Skeleton`** (Skeleton.txt, 139 lines):

- **Synched** (31–42, static 132–138): **16 `DATA_STRAY_CONVERSION_ID`, `BOOLEAN`, false**.
- **The freezing tracker** (constructor 7–29): a `ConversionTracker` with that accessor,
  converts to **`stray`**, level event **1048**, afflicted while **`isInPowderSnow`** (the
  field, lambda 115–119), keys **`FreezingTime`** (140) and **`StrayConversionTime`** (300),
  no post-conversion step. `tick` (51–58): **the tracker first, then `AbstractSkeleton.tick`**
  (the zombie's ticks its after `Monster.tick`): the tracker reads the powder snow flag the
  last tick's move left, and when it converts, the discarded skeleton's own tick still runs
  in that call (nothing returns early). Saved and read with the tracker (60–80).
  `startFreezeConversion(t)` (82–88) is the tracker's `startConversion`.
- **`canFreeze`** false (90–93): a skeleton in powder snow takes no freezing hurt
  (Clustine's `living.rs` 585 asks `can_freeze`; `mobs/body.rs` 600 sets it from
  `#freeze_immune_entity_types`, which holds the stray and not the skeleton: the skeleton
  needs this override; the bogged and the parched do not have it and freeze as any mob).
- **`isShaking`** (44–49) is the tracker's converting flag: the client shakes it (**Client**).
- **Sounds** (95–113): `skeleton_ambient`, `skeleton_hurt`, `skeleton_death`, `skeleton_step`.
- Timing and what is copied: 11.5. A skeleton standing in powder snow for 140 ticks shakes
  for 301 more and becomes a stray holding its bow and wearing what it wore.

**`Stray`** (87 lines): nothing of its own but sounds (`stray_*`, 44–62), its arrows' slowness
600 (`getArrow`, 64–86) and the spawn rule (11–42, N9: up through powder snow, then the
monster rule and, unless a spawner, the sky seen). In `#freeze_immune_entity_types`.

**`Bogged`** (240 lines) `implements Shearable`:

- Health 16 (9–15); attack intervals 50 and 70 (172–180); arrows with poison 100 (148–170).
- **Synched** 16 `DATA_SHEARED`, `BOOLEAN`, false (25–36, static 233–239). **Saved**
  `sheared` (38–48), read with default false (50–61).
- **`mobInteract`** (83–126): shears and `readyForShearing()` (not sheared, 212–220): on the
  server `shear(level, PLAYERS, stack)`, game event `shear` with the player, the shears
  `hurtAndBreak(1, player, hand's slot)`; `SUCCESS` on both sides; else
  `AbstractSkeleton`'s.
- **`shear(level, source, stack)`** (182–199): `level.playSound(null, this, bogged_shear,
  source, 1, 1)`; the table `shearing/bogged` dropped by `dropFromShearingLootTable`, each
  item `spawnAtLocation` at **`getBbHeight()`** (the box's top, not the eye, 201–231);
  `setSheared(true)`. The table (`loot_table/shearing/bogged.json`): one pool, two rolls, each
  a brown or a red mushroom (equal weights, count 1), random sequence
  `shearing/bogged`. A dispenser's shears do the same (`Shearable`; Clustine's dispensers
  have no shears).
- Sounds `bogged_ambient`, `bogged_hurt`, `bogged_death`, `bogged_step`, `bogged_shear`.
  No `finalizeSpawn` of its own: `AbstractSkeleton`'s.

**`Parched`** (85 lines): health 16 (35–41); intervals 50 and 70; arrows with weakness 600
(11–33); immune to weakness (`canBeAffected`, 73–84); sounds `parched_*`. Nothing about
riding: it rides a camel husk only because `Husk.finalizeSpawn` makes it so for `NATURAL`
(n7 Husk.txt 93–257: the parched made with `NATURAL`, snapped to the husk's position and
yaw, finalised with the husk's arguments, riding the camel husk, added with its
passengers): N9's.

**`WitherSkeleton`** (158 lines), only as the others call it: nothing of it is reached from
the four above.

### 11.3 The creeper (Creeper.txt, 691 lines)

- **Construction** (27–39): `maxSwell` 30, `explosionRadius` 3. No draw.
- **Synched** (200–223, static 676–690): **16 `DATA_SWELL_DIR`, `INT`, −1**; **17
  `DATA_IS_POWERED`, `BOOLEAN`, false**; **18 `DATA_IS_IGNITED`, `BOOLEAN`, false**. `swell`
  and `oldSwell` are fields, not synched (the client counts its own from the direction).
- **Saved** (225–252): `powered` (boolean), `Fuse` (short, `maxSwell`), `ExplosionRadius`
  (byte), `ignited` (boolean). **Read** (254–287): `powered` into the synched value (default
  false), `Fuse` default 30, `ExplosionRadius` default 3, and `ignite()` if `ignited`.
- **Goals** (41–140): 1 `FloatGoal`; 2 `SwellGoal`; 3 `AvoidEntityGoal(Ocelot, 6.0f, 1.0,
  1.2)`; 3 `AvoidEntityGoal(Cat, 6.0f, 1.0, 1.2)`; 4 `MeleeAttackGoal(1.0, false)`; 5
  `WaterAvoidingRandomStrollGoal(0.8)`; 6 `LookAtPlayerGoal(Player, 8.0f)`; 6
  `RandomLookAroundGoal`; targets 1 players, 2 `HurtByTargetGoal()`. **It floats**, and it
  runs from cats and ocelots within 6 without any target (Clustine has both).
- **`SwellGoal`** (SwellGoal.txt): flags `MOVE` (7–18), every tick (67–70). `canUse` (20–45):
  `getSwellDir() > 0`, or a target alive within 3 (distance² < 9). `start` stops the
  navigation and keeps the target; `tick` (72–115): no target or a dead one → direction −1;
  beyond 7 (distance² > 49) → −1; not seen (`Sensing.hasLineOfSight`) → −1; else +1.
- **`tick`** (289–345), while alive:
  ```
  oldSwell = swell
  if isIgnited(): setSwellDir(1)
  dir = getSwellDir()
  if dir > 0 && swell == 0: playSound(creeper_primed, 1.0f, 0.5f); gameEvent(PRIME_FUSE)
  swell += dir; if swell < 0: swell = 0
  if swell >= maxSwell: swell = maxSwell; explodeCreeper()
  Monster.tick()
  ```
- **With no target**: the direction stays −1 and `swell` 0; nothing is drawn by the creeper
  itself. **A fall** swells it: `causeFallDamage` (167–198) adds `(fallDistance × 1.5) as i32`
  to `swell`, capped at `maxSwell − 5` (25), which then runs down by one a tick: no sound, no
  explosion, nothing the client sees (the field is not synched). `getMaxFallDistance`
  (150–165) is `getComfortableFallDistance(0)` without a target (paths' drops).
- **A player's flint and steel or fire charge** (`mobInteract`, 464–528), after
  `Mob.interact`'s name tag, egg and lead: an item in `#creeper_igniters` (flint and steel,
  fire charge): `level.playSound(player, getX(), getY(), getZ(), fire charge ?
  firecharge_use : flintandsteel_use, getSoundSource(), 1.0f, random.nextFloat() × 0.4f +
  0.8f)` (the creeper's own random, both sides; the player is the one left out, **Inferred**
  from `Level.playSound`'s first argument); on the server `ignite()` (the synched flag) and
  the item used: a damageable one `hurtAndBreak(1, player, hand's slot)`, else `shrink(1)`
  (a creative player keeps both, **Inferred**: `Player.interactOn` restores a creative
  player's count, n5 Player.txt, not re-read here); `SUCCESS`. From then `tick` sets the
  direction to 1 every tick; `SwellGoal` runs (the direction is above 0), stops the
  navigation and, having no target, sets −1 again in the goal tick, which the next
  `Creeper.tick` overwrites before counting: `swell` rises by one a tick, `creeper_primed`
  plays at the first, and at 30 **the creeper explodes**, 30 ticks after the click.
- **`explodeCreeper`** (530–574), server: `dead = true`; `level.explode(this, getX(), getY(),
  getZ(), explosionRadius × (powered ? 2 : 1), ExplosionInteraction.MOB)`;
  `spawnLingeringCloud()` (576–642: only with active effects, an `AreaEffectCloud` of radius
  2.5, never in N7k); `triggerOnDeathMobEffects(KILLED)`; `discard()` (no death, no loot).
  `MOB` breaks blocks only with `mob_griefing`, and then their drops decay by
  `mob_explosion_drop_decay` (n5 ServerLevel.txt 2568–2600). Clustine has the explosion
  (`region/explosions.rs` 195, TNT's interaction).
- **Lightning** (`thunderHit`, 450–462): `Monster`'s, then `powered` true. None in N7k.
- **`setTarget`** (347–356) refuses a goat. `doHurtTarget` (397–400) is true and does
  nothing: the creeper's harm is its explosion. A charged creeper's kill drops the
  `charged_creeper` table once (`killedEntity`, 368–395): N9.
- **Sounds**: hurt `creeper_hurt`, death `creeper_death` (358–366), `creeper_primed`; **no
  ambient sound** (none defined by `Creeper` or `Monster`, Monster.txt 44–72).
- **`finalizeSpawn`**: none of its own: `Monster`'s, the Tail only.

### 11.4 The spiders (Spider.txt, 395 lines; CaveSpider.txt, 96 lines)

- **Synched** (121–132, static 388–394): **16 `DATA_FLAGS_ID`, `BYTE`, 0; bit 0 (mask 1)
  climbing** (`isClimbing`, 214–228; `setClimbing`, 230–258, sets or clears that bit only,
  `& −2`). No saved data of its own.
- **Climbing.** `tick` (134–146): `Monster.tick()` (the whole tick, movement included), then
  **on the server `setClimbing(horizontalCollision)`**: the flag is whether the move just made
  ran into something sideways. `onClimbable()` (183–187) **is that flag**, not the block, and
  the next tick's travel reads it (n5 LivingEntity.txt): in
  `handleRelativeFrictionAndCalculateMovement` (7154–7253) `(horizontalCollision || jumping)
  && onClimbable()` sets the motion's y to 0.2; `handleOnClimbable` (7254–) resets the fall
  distance, clamps x and z to ±0.15 (`0.15000000596046448`) and y to at least −0.15. So a
  spider walking into a wall rises 0.2 a tick and does not fall faster than 0.15 while it
  touches one. Clustine's body asks the block (`physics/body.rs` 524 `climbable`): the spider
  needs a type hook for `onClimbable`, and its `isPushable`, which reads it (n5
  LivingEntity.txt 9141–), the same.
- **Navigation**: `WallClimberNavigation` (112–119; n7 WallClimberNavigation.txt, dumped:
  `createPath`, `moveTo` towards an entity, `tick`). Clustine has no such kind
  (`mobs/ai/navigation.rs` 22–31).
- **Cobwebs** do not slow it (`makeStuckInBlock`, 189–199); poison does not affect it
  (`canBeAffected`, 201–212).
- **Goals** (15–110): 1 `FloatGoal`; 2 `AvoidEntityGoal(Armadillo, 6.0f, 1.0, 1.2, not
  scared)` (the predicate, 377–386: `!isScared()`); 3 `LeapAtTargetGoal(0.4f)`; 4
  `SpiderAttackGoal`; 5 `WaterAvoidingRandomStrollGoal(0.8)`; 6 `LookAtPlayerGoal(Player,
  8.0f)`; 6 `RandomLookAroundGoal`; targets 1 `HurtByTargetGoal()`, 2 and 3
  `SpiderTargetGoal(Player)` and `(IronGolem)`, which refuse when the light's magic value at
  the spider is 0.5 or more (Spider$SpiderTargetGoal.txt 12–26). `SpiderAttackGoal` (its
  file 12–50) needs a target; in light it gives the target up with `nextInt(100) == 0` (own),
  only while running. `LeapAtTargetGoal` (LeapAtTargetGoal.txt 25–60): no controlling
  passenger, a target 2 to 4 away, on the ground, then `nextInt(reducedTickDelay(5))`: no
  target, no draw. So without a target a spider floats, flees uncurled armadillos, strolls
  and looks.
- **`finalizeSpawn`** (260–351), R the caller's:
  ```
  group = Monster.finalizeSpawn(…)                     // the Tail
  if R.nextInt(100) == 0:
      s = SKELETON.create(level, JOCKEY)
      s.snapTo(getX(), getY(), getZ(), getYRot(), 0)
      s.finalizeSpawn(level, difficulty, reason, null) // 11.2's draws, from R
      s.startRiding(this, false, false)                // not added here (below)
  if group == null:
      group = SpiderEffectsGroupData()
      if level.getDifficulty() == HARD && R.nextFloat() < 0.1f × s:
          group.setRandomEffect(R)
  if group is SpiderEffectsGroupData with an effect e:
      addEffect(MobEffectInstance(e, −1))              // for ever
  ```
  The skeleton is only made a passenger: it enters the world with its spider, by the
  caller's `addFreshEntityWithPassengers` (**Inferred**: `EntityType.spawn` and the natural
  spawner add with passengers; the drowned's nautilus and the zombie's chicken, vehicles,
  are added explicitly). **`setRandomEffect`** (Spider$SpiderEffectsGroupData.txt 11–44):
  `i = R.nextInt(5)`: 0 and 1 speed, 2 strength, 3 regeneration, 4 invisibility. **The effect
  draw is made only on hard**; the jockey's `nextInt(100)` at every difficulty and reason.
  A group handed in that is not the spider's (a spawner's) gives no effect.
- **Riding**: a passenger narrower than the spider sits at (0, 0.3125 × scale, 0)
  (`getVehicleAttachmentPoint`, 353–375); the cave spider's 0.21875 (73–95).
- **Sounds** (159–181): ambient `spider_ambient`, hurt `spider_hurt`, death `spider_death`,
  step `spider_step` at 0.15, 1.
- **The cave spider's differences**: health 12 (11–17); **`finalizeSpawn` returns the group
  handed in and does nothing else** (68–71): **no Tail** (no follow-range triangle, never
  left-handed), no jockey, no effect, **no draw at all**; a hit poisons for 7 seconds on
  normal and 15 on hard (`doHurtTarget`, 19–66; N9). Everything else is the spider's.

### 11.5 `ConversionTracker` (ConversionTracker.txt, 281 lines) and `Mob.convertTo`

- **State**: `afflictionTime` (0 at construction), `conversionTime`; the converting flag is
  **the synched boolean** handed in (the zombie's 18, the skeleton's 16), not a field.
- **`tick`** (70–132):
  ```
  if !(level is ServerLevel) || !mob.isAlive() || mob.isNoAi(): return   // nothing changes
  if isAfflicted():
      if isConverting():
          conversionTime -= 1
          if conversionTime < 0: doConversion(level)
      else:
          afflictionTime += 1
          if afflictionTime >= totalAfflictionTime: startConversion(totalConversionTime)
  else:
      afflictionTime = −1; setConverting(false)
  ```
  `startConversion(t)` (232–240): `conversionTime = t`, converting true.
- **So, for a zombie** (`isEyeInFluid(WATER)`, n7 Zombie.txt 1382–1388; 600 and 300; ticked
  after `Monster.tick`, 410–421) **that has never left water**: converting from its 600th
  tick with the eye under, and converted on the **301st** tick after that: **901 ticks**. Once
  it has been out of water, `afflictionTime` is −1 and the count needs **601**. Leaving water
  at any moment, converting or not, resets both: `afflictionTime` −1 and the synched flag
  false. A dead or `NoAI` mob's counts stand still. **A skeleton**: 140 and 300 with
  `isInPowderSnow`, 441 ticks from fresh (442 after leaving once).
- **Saved** (158–183): the conversion key = `conversionTime` if converting, else **−1**; the
  affliction key = `afflictionTime` **if afflicted at the moment of saving**, else −1 (the
  condition is evaluated then). **Read** (185–210): affliction default 0; a conversion value
  other than −1 restarts the conversion with it (the flag set), else the flag cleared.
- **`doConversion`** (212–230): `mob.convertTo(convertsTo, ConversionParams.single(mob, true,
  true), after)` (keep equipment, keep the loot pick-up flag; the team the mob's, n7
  ConversionParams.txt 29–39); `after` (256–280): the post step if any (the zombie's:
  `handleAttributes(specialMultiplier at the new mob's position, CONVERSION)` on **the new
  mob**, n7 Zombie.txt 1370–1381: its own random, four draws at special 0 — reinforcement,
  knockback, follow range, leader), then, unless the new mob is silent,
  `level.levelEvent(null, sound, newMob.blockPosition(), 0)`: the zombie's
  `getConversionSound` 1040 (husk 1041, n7 Husk.txt 83–86), the skeleton's 1048. The two
  suppliers the zombie hands in are method references (no lambda bodies; **Inferred**
  `convertsToWhenDrowning` and `getConversionSound` from their types).
- **`Mob.convertTo(type, params, reason = CONVERSION, after)`** (n7 Mob.txt 2976–3023): null
  if the mob is removed; `type.create(level, CONVERSION)` (the new type's constructor and
  its construction draws); `params.type().convert(old, new, params)`; `after.finalizeConversion
  (new)`; `level.addFreshEntity(new)`; and, for `SINGLE`, `old.discard()`. **No
  `finalizeSpawn`**, and nothing else is drawn.
- **What `SINGLE` copies** (n7 ConversionType$1.txt 12–144, then `convertCommon`, n7
  ConversionType.txt 47–269), old to new, in this order: the position and rotation
  (`copyPosition`), the motion; the old one's first passenger is taken off and seated on the
  new one (the new one's own passengers discarded first); the old one's vehicle is left and
  the new one rides it; with `keepEquipment` every slot of `EquipmentSlot.VALUES` that holds
  something is moved (`copyAndClear`) with its drop chance; the fall distance, the
  fall-flying flag (shared flag 7), `lastHurtByPlayerMemoryTime`, `hurtTime`, `yBodyRot`,
  `onGround`; the sleeping position; the leash (`setLeashedTo(holder, true)`). Then
  `convertCommon`: absorption; every active effect (a copy); the baby flag (set only if the
  old one is a baby); for two ageable mobs the age and forced age; the `angry_at` memory
  where both brains have it; the loot pick-up flag (with `preserveCanPickUpLoot`);
  left-handed; `NoAI`; persistence (set only if the old one had it); custom name visible;
  **the shared on-fire flag only** (`setSharedFlagOnFire(isOnFire())`, not the fire ticks);
  permanently invulnerable; invulnerable time; no gravity; portal cooldown; silent; every
  entity tag; the data components `custom_name` and `custom_data` (static 270–293); the
  team; and a zombie that can break doors makes a new zombie able to. **Not copied: health**
  (the new mob has its own maximum), air, fire ticks, attributes and their modifiers, the
  UUID (**Inferred**: a new entity's own). `SPLIT_ON_DEATH` (n7 ConversionType$2.txt) is the
  slime's.

### 11.6 `Crackiness` (Crackiness.txt, 99 lines; Crackiness$Level.txt, 87 lines)

Levels `NONE`, `LOW`, `MEDIUM`, `HIGH` (ordinals 0 to 3). `byFraction(f)` (28–52): `f <
fractionHigh` → `HIGH`; else `f < fractionMedium` → `MEDIUM`; else `f < fractionLow` → `LOW`;
else `NONE` (all strict, `f32`). **`GOLEM` = (0.75, 0.5, 0.25)** (static 82–98): none at 75 %
of the maximum health or more, low below 75 %, medium below 50 %, high below 25 %. (`WOLF_ARMOR`
is (0.95, 0.69, 0.32), by durability, `byDamage`, 54–80.) The iron golem plays
`iron_golem_damage` when a hurt changes the level (1.2).

### 11.7 `BlockPattern` and its parts

- **Building** (BlockPatternBuilder.txt): `aisle(rows…)` adds one aisle (37–144; every row
  the first row's width, every aisle the first aisle's height); `where(c, predicate)`
  (153–168); the space is always true (constructor 13–35, lambda 264–267); `build` makes
  `pattern[aisle][row][column]` (179–262): **depth** = the number of aisles, **height** = rows,
  **width** = columns. The golems' patterns are one aisle each (1.5): depth 1, height 3, width
  1 (snow) or 3 (iron).
- **Addressing** (`translateAndRotate(origin, forwards, up, column, row, aisle)`, BlockPattern.txt
  259–344): `up` equal or opposite to `forwards` is refused. With `F` = `forwards`' step, `U` =
  `up`'s, and `L = F × U` (the vector cross product, `Vec3i.cross`):
  `cell = origin + L × column − U × row + F × aisle`. So columns run along `F × U`, rows run
  **down** `U`, aisles along `F`. `BlockPatternMatch.getBlock(x, y, z)` (BlockPattern$BlockPatternMatch.txt
  80–96) is the cached `BlockInWorld` at `translateAndRotate(frontTopLeft, forwards, up, x, y,
  z)`: `x` the column, `y` the row from the top, `z` the aisle. The golem is placed at
  `getBlock(c, 2, 0)` (1.5): the bottom row's middle cell, along `−U` from the head.
- **`find(level, pos)`** (156–246):
  ```
  cache = createLevelCache(level, false)
  size = max(width, height, depth)                         // 3 for both golems
  for p in BlockPos.betweenClosed(pos, pos + (size−1, size−1, size−1)):   // x fastest, then y, then z
      for forwards in Direction.values():                  // DOWN, UP, NORTH, SOUTH, WEST, EAST
          for up in Direction.values():
              if up == forwards || up == forwards.opposite(): continue
              m = matches(p, forwards, up, cache)
              if m != null: return m
  return null
  ```
  `betweenClosed`'s order is `x = index % w`, `y = index / w % h`, `z = index / w / h` (n7
  BlockPos$4.txt). **Every one of the 24 orientations is tried at each of the 27 corner
  cells**, and the first match is the one used: upright, upside down (the head below), lying
  on the ground facing any way (`forwards` up or down), and on its side. The candidates are
  only at non-negative offsets from the placed head; the orientations make every placement
  reachable (**Inferred**: with the cells running `+L` and `−U`, some orientation puts the
  corner at such an offset; for the upright iron golem, `forwards` south with `up` up gives
  `L` = west and the corner at the head + (1, 0, 0)).
- **`matches(origin, forwards, up, cache)`** (93–154): for column `i` (outer), row `j`, aisle
  `k` (inner): `pattern[k][j][i].test(cache[translateAndRotate(origin, forwards, up, i, j,
  k)])`, the first failure returning null; else a `BlockPatternMatch(origin, forwards, up,
  cache, width, height, depth)`.
- **The cells** (BlockInWorld.txt; BlockPattern$BlockCacheLoader.txt 19–29): one
  `BlockInWorld` per position, made on first use and kept for the search. `getState()`
  (31–54) reads the block only if `loadChunks` (false for `find`) or the chunk is loaded;
  otherwise it stays null. `hasState(p)` (90–108) is `p.test(cell.getState())`.
- **The `~` predicate** of the iron golem's patterns: `BlockInWorld.hasState(…)` of the
  `invokedynamic` #2 at n7 CarvedPumpkinBlock.txt 465 and 506. CarvedPumpkinBlock's synthetic
  lambdas are #0, #1, #3, #4 and #5 (lines 613–653); **#2 has no lambda body: it is a method
  reference**, whose target `javap -c` does not print (the bootstrap arguments need `-v`).
  **Inferred**: `BlockStateBase::isAir` (the game's long-standing `~`), so the two shoulder
  cells beside the head and the two cells beside the bottom block must be air (any air:
  cave and void air too). The snow golem's patterns have no `~`.

### 11.8 `ZombieGroupData`, `HuskGroupData` and the turtle egg goal

- **`Zombie$ZombieGroupData(isBaby, canSpawnJockey)`** (7–17): two final booleans and nothing
  else; `Zombie.finalizeSpawn` makes it `(getSpawnAsBabyOdds(R), true)` when no group is handed
  in (n7 Zombie.txt 976–984). **A group of another class skips the baby, the jockey, the
  doors, the equipment and its enchantment** (n7 Zombie.txt 985–987: a failed `instanceof`
  jumps past them, and only the Halloween head and `handleAttributes` follow).
- **`Husk$HuskGroupData(ZombieGroupData)`** (5–16): copies the two booleans,
  `triedToSpawnCamelHusk` false; the husk sets it (3.4.2).
- **`Zombie$ZombieAttackTurtleEggGoal`** (Zombie$ZombieAttackTurtleEggGoal.txt, 62 lines), at
  priority 4 for every zombie, husk, zombie villager and drowned: a `RemoveBlockGoal(turtle_egg,
  mob, 1.0, 3)` (5–19), which is a `MoveToBlockGoal(mob, 1.0, 24, 3)` (n7 RemoveBlockGoal.txt
  11–26). **It needs no target.** `canUse` (n7 RemoveBlockGoal.txt 27–77): `mob_griefing`;
  a countdown; at 0 `findNearestBlock` (a turtle egg within 24 horizontally and 3 vertically,
  `MoveToBlockGoal`'s search, n7); found: the countdown `reducedTickDelay(20)` and true; not
  found: the countdown `nextStartTick(mob)` = `reducedTickDelay(200 + nextInt(200))`, **an own
  draw** (n7 MoveToBlockGoal.txt 91–100). So every zombie draws `nextInt(200)` about every
  100 to 200 ticks with no egg near, and walks to any egg within 24. At the egg (n7
  RemoveBlockGoal.txt 96–284) it hops (±0.3), sends particles with three own `nextFloat()`,
  plays `zombie_destroy_egg` (0.5, 0.9 + own `nextFloat()` × 0.2, `HOSTILE`, 21–38) every 6
  ticks, and after 60 removes the egg, with 20 particles of three `nextGaussian()` each and
  `turtle_egg_break` (0.7, 0.9 + the level's `nextFloat()` × 0.2, `BLOCKS`, 40–56); it stops
  within 1.14 (`acceptedDistance`, 58–61). Clustine has turtles and their eggs
  (`types/turtle.rs`) and `MoveToBlockGoal` (`mobs/ai/goals.rs` 740–).

### 11.9 The enchantment providers

- **`by_cost_with_difficulty`** (EnchantmentsByCostWithDifficulty.txt 28–72): `cost =
  Mth.randomBetweenInclusive(r, minCost, minCost + (int)(s × maxCostSpan))` (one `nextInt`),
  then `EnchantmentHelper.selectEnchantment(r, stack, cost, the set's stream)` (n7
  EnchantmentHelper.txt 1171–1280, dumped: the item's enchantability, the cost's own draws,
  weighted picks repeated while a draw against the cost passes, incompatible ones filtered)
  and each result `upgrade`d into the stack's enchantments. `mob_spawn_equipment` is (5, 17,
  `#on_mob_spawn_equipment`): at special 0 the cost is 5 and still drawn — but the provider
  is never reached there, since `enchantSpawnedEquipment`'s `nextFloat() < chance × s` fails
  first (3.2).
- **`by_cost`** (EnchantmentsByCost.txt 21–51): `cost.sample(r)` then the same selection.
- **`single`** (SingleEnchantment.txt 21–42): the level `level.sample(r)` clamped to the
  enchantment's minimum and maximum; `pillager_spawn_crossbow` is Piercing at a constant 1
  (no draw: **Inferred**, a constant provider draws nothing).
- **`VanillaEnchantmentProviders`** (141 lines): `mob_spawn_equipment`,
  `pillager_spawn_crossbow`, `raid/pillager_post_wave_3`, `raid/pillager_post_wave_5`
  (Quick Charge), `raid/vindicator`, `raid/vindicator_post_wave_5` (Sharpness),
  `enderman_loot_drop` (Silk Touch); their data is the datagen's
  `enchantment_provider/*.json`.
- **What building them needs**, for normal and hard: the enchantment registry's data
  (`enchantment/*.json`: weight, minimum and maximum cost per level, maximum level, supported
  and primary items, exclusive sets) and the order of `#on_mob_spawn_equipment`'s members as
  the stream gives them; each item's `enchantable` component value; `selectEnchantment` and
  `getAvailableEnchantmentResults` ported from n7's EnchantmentHelper; enchantments stored on
  an item stack (the `enchantments` component) and shown to clients in `SetEquipment` and
  dropped items; and, because the rest of the game reads them, what each enchantment does
  on a mob's weapon or armour (N9's attacks). Not dumped: `Enchantment` itself
  (`getMinCost`, `getMaxCost`, `isPrimaryItem`), whose rules are data-driven
  (**Inferred** from the JSON). Refusing normal and hard until then (P 1.4) keeps every
  draw of 3.2 reachable and none of these.

### 11.10 The snowball (Projectile.txt, ThrowableProjectile.txt, ThrowableItemProjectile.txt, Snowball.txt)

For open question 2, what a snow golem's snowball does, not a port:

- **Thrown** by `Projectile.spawnProjectileUsingShoot` (Projectile.txt 515–547): made at the
  owner's eye less `0.10000000149011612` (ThrowableItemProjectile.txt 30–40),
  `shoot(dx, dy, dz, 1.6f, 12.0f)` (302–349, `getMovementToShoot` 263–301): the direction
  normalised, plus `triangle(0, 0.0172275 × uncertainty)` on each axis (three draws of the
  projectile's own random, **Inferred** `this.random`), times the speed; the rotation from
  the motion.
- **Flies** (ThrowableProjectile.txt `tick` 69–117, `applyInertia` 118–182): the hit along the
  move (`ProjectileUtil.getHitResultOnMoveVector`, not dumped), the motion × 0.99 (0.8 in
  water, 183–187), gravity 0.03 (205–208).
- **Hits** (Snowball.txt): an entity is hurt by `thrown(snowball, owner)` for **0**, or 3 for
  a blaze (`onHitEntity`, 86–111); on any hit, entity or block, entity event 3 (eight snowball
  particles, 56–84) and `discard()` (`onHit`, 113–129). A hurt of 0 still knocks back
  (**Inferred**: the hurt is taken and the knockback, B 1.4, follows from the snowball's
  direction, Projectile.txt 1041–1055): **the knockback is what keeps zombies off a snow
  golem**. A port needs the projectile entity, its tracking, the hit test against blocks and
  entities (also across a line), and the hurt by a projectile: N9's model.

### 11.11 What N7k's bodies touch of the rest

- **`MobEffects`** (541 lines): the effects in registration order, speed, slowness, haste,
  mining fatigue, strength, instant health, … oozing (40; the registry ids follow,
  **Inferred**). N7k names: speed, strength, regeneration, invisibility (spider), poison,
  weakness, slowness (arrows), instant health, fire resistance, water breathing (witch),
  hunger (husk), mining fatigue (elder guardian).
- **`Potions`** (905 lines): the witch's four by name: `water_breathing` (water breathing
  3600, 494–), `fire_resistance` (3600, 275–), `healing` (instant health, 528–), `swiftness`
  (speed 3600, 309–); the trader's `invisibility` (3600, 189–). Instant health heals 4 at
  level I (**Inferred**, `InstantenousMobEffect` not dumped).
- **`TradeSets`** (740 lines): sets `trade_set/<profession>/level_<n>` for the 13 working
  professions and `wandering_trader/buying`, `common`, `uncommon` (535–541): their data is
  the datagen's `trade_set/` and `villager_trade/`. **`TradeSet`**, **`MerchantOffers`**
  (an `ArrayList` of offers, `getRecipeFor`, `copy`): N10's.
- **`GossipContainer`**: a villager's gossips (`add`, `decay`, `transferFrom`, `copy`); empty
  in N7k.
- **`Raid`**: `joinRaid` (1377–1441) is what a raid sets on a raider it spawns: its wave
  (`addWaveMob`), current raid, wave number, `canJoinRaid`, ticks outside 0, the position,
  **`finalizeSpawn` with `EVENT`** (so no patrol leader draw), `applyRaidBuffs`, on the ground,
  added with passengers. `getOminousBannerInstance` (1649–1655, 1569–1636): a banner with
  eight patterns (rhombus middle cyan, stripe bottom light grey, stripe centre grey, border
  light grey, stripe middle black, half horizontal light grey, circle middle light grey,
  border black), patterns hidden from the tooltip, item name "Ominous Banner", rarity
  uncommon. **`Raids`**: `canJoinRaid` (155–171), `getNearbyRaid`. N10.
- **`RangedAttackMob`** (16 lines): `rangedAttackUncertainty` defaults to `14 − 4 ×
  difficulty id` (peaceful 14, easy 10, normal 6, hard 2) as `f32`.
- **`RangedCrossbowAttackGoal`**: `canUse` (44–55) needs a valid target and a crossbow:
  nothing without a target.

### 11.12 Section 5's rows for the types dumped now

S below is `AbstractSkeleton.finalizeSpawn` (11.2): Tail; Armour; Ench(main hand, then the
armour slots that hold something); R `nextFloat()` (< 0.55 s: loot); on Halloween with an
empty head R `nextFloat()` (< 0.25) and then R `nextFloat()` (< 0.1). D is the drowned's
equipment: R `nextFloat()` (> 0.9 as `f64`), and if it passes R `nextInt(16)`.

| Type | `SPAWN_ITEM_USE` (egg) | `STRUCTURE` | `NATURAL` (N9) | Notes |
|---|---|---|---|---|
| drowned | Z with D in step 6 (no Armour, no zombie weapon), then R `nextFloat()` (< 0.03: shell) if the off hand is empty | the same, then with a trident R `nextFloat()` (< 0.5), and an adult outside rivers makes a zombie nautilus (persistent) finalised from R with its own draws | as `STRUCTURE`, the nautilus not persistent | ocean ruins (L 1.2, D3); Ench(main hand) only when D gave a weapon |
| skeleton, stray, bogged | S | — (no template holds one; spawners are N9's) | S (the stray's spawn rule's draws are N9's) | the bow set without a draw |
| parched | S | — | as the husk's rider, S (n7 Husk.txt 270–338) | not alone in `NATURAL` |
| creeper | Tail | — | Tail | |
| spider | Tail; R `nextInt(100)` (0: a skeleton, `JOCKEY`, with S from R and the spider's reason); on **hard** only R `nextFloat()` (< 0.1 s) and then R `nextInt(5)` | — | the same | the effect is drawn only on hard |
| cave spider | **none** | — | none | no Tail |

At `s = 0` an egg's skeleton draws Tail (3), Armour (1), Ench of the bow (1), loot (1),
[Halloween 1–2]: six; a drowned Z's draws with D (1, and 1 more one time in ten) in place of
Armour and the weapon, Ench(main hand) only with a weapon, then the shell (1); a spider four,
with a jockey's skeleton's six more one time in a hundred; a creeper three; a cave spider
none.

### 11.13 For the plan

Where `n7k-plan.md` is wrong or incomplete against what is read here:

1. **Floating** (plan 1.3, 3): the skeleton family has **no `FloatGoal`** (11.2): it sinks as
   the zombies do. The creeper and the spiders float. The table's "skeleton, stray, bogged,
   parched | their float" is wrong.
2. **The drowned swims only at night** (or when searching for land, 11.1): by day in water it
   sinks and walks the bottom; by day on land it walks to water within 10 (up to 30 own draws
   per ask); at night it swims up and walks out onto land. "A drowned swims" needs that
   precision, and it needs `DrownedMoveControl`, its `travelInWater`, the swimming pose, and
   `AmphibiousPathNavigation` (which Clustine has).
3. **The drowned's weapon does not depend on the difficulty** (11.1): at easy one drowned in
   16 holds a trident and about one in 27 a fishing rod, one in 33 a nautilus shell. Plan 1.3's
   list of what is worn and held at easy omits it; and for ocean ruins (D3, `STRUCTURE`) half
   the trident holders outside rivers ride a new zombie nautilus.
4. **Things monsters do without a target** that the plan's "float, stroll and look" misses,
   each ordinary play with animals Clustine has: skeletons run from wolves (6) and, burning in
   daylight, flee to shade (`FleeSunGoal`) and keep out of the sun (`RestrictSunGoal`, the
   navigation's `avoidSun` trim, which `navigation.rs` names as N9's); creepers run from cats
   and ocelots; spiders from armadillos that are not rolled up; **every zombie, husk, zombie
   villager and drowned seeks turtle eggs within 24 and tramples them** (11.8), drawing
   `nextInt(200)` about every 100 to 200 ticks whether or not an egg is near. A body without
   these draws diverges from the game's streams at once.
5. **The creeper explodes in ordinary play** (plan 1.3 says it does not swell): flint and
   steel or a fire charge ignites it, and it explodes 30 ticks later with radius 3 (11.3).
   Clustine has explosions; the plan has to decide: the ignition and the explosion with
   `ExplosionInteraction.MOB` (blocks only with `mob_griefing`, decaying drops), or a named
   refusal (the item used for nothing). A fall's swelling is server-only and harmless.
6. **The spider's climbing** needs a type hook for `onClimbable` (the synched flag, set from
   `horizontalCollision` after each tick, 11.4) in the body's travel and pushing, and
   `WallClimberNavigation` (n7, dumped) for its paths; the plan names neither.
7. **The spider's effects are drawn only on hard** (11.4); plan 3's "effects are drawn,
   applied never" is wrong at easy (no draw). The jockey (one in 100, every reason and
   difficulty) makes a skeleton that is added as the spider's passenger by the caller, not by
   `finalizeSpawn`; the egg's path must add passengers. **The cave spider draws nothing at
   all** (no Tail).
8. **Conversion** (plan 2.4): `convertTo` calls **no `finalizeSpawn`**; the zombie's step is
   `handleAttributes` with `CONVERSION` on the new mob (its own four draws), the skeleton's
   nothing. **Health is not copied** (the new mob starts at its maximum), nor fire ticks
   (only the flag), air or attribute modifiers; the full list of what is copied is 11.5's,
   including a passenger moved to the new mob and the new mob seated on the old one's
   vehicle (a baby zombie on a chicken converts riding it). The new mob is added before the
   old one is discarded; the level event is played at the new mob's cell unless it is
   silent. The timing is 600 + 301 ticks (601 + 301 after the zombie has once left water);
   "after 30 seconds … over 15 more" is right to the tick only so.
9. **The skeleton's freezing** (plan 3): 140 + 301 ticks in powder snow (11.2, 11.5); the
   skeleton's tracker runs **before** its tick; `Skeleton.canFreeze` false (no freezing
   hurt) while the bogged and the parched freeze; the stray keeps the bow and armour.
10. **The bogged's shearing** is not the snow golem's: drops at the box's top, `bogged_shear`
    from the player's source, two rolls of brown or red mushroom (11.2); `shearing/bogged`
    must be extracted.
11. **Golem patterns** (plan 2.6): the search order is 11.7's (27 corner cells, x fastest;
    `forwards` and `up` in `Direction` order, 24 pairs), and upside-down golems are built
    too. The `~` predicate is not readable from this dump (11.7): take air, as **Inferred**,
    and let the comparison with the official server (plan 6.2) settle it, with a golem built
    with a non-air block (a torch, water) at a shoulder.
12. **Crackiness** (plan 1.1): thresholds 0.75, 0.5, 0.25, strict (11.6).
13. **The snowball** (open question 2): the knockback of a 0-damage hurt is what makes a snow
    golem useful against zombies (11.10). The recommendation stands as a named difference,
    but the plan should say that the snow golem then neither damages nor pushes anything.
14. **Difficulty** (plan 1.4): besides the special multiplier, the difficulty's id sets the
    ranged uncertainty (11.11) and the skeletons' attack intervals (`reassessWeaponGoal`):
    N9's, but the setting is read by them from N7k.
15. **Section 5 of the plan** should cite 11.12 for the types dumped now; the spider's
    jockey and the drowned's nautilus are vehicles or passengers made inside
    `finalizeSpawn` and need the egg path to add them in the game's order.
