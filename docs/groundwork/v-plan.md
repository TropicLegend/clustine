# V: survival mode, planned

**Agreed by the owner on 2026-11-03**, with every recommendation of section 11 but one:
formats rise at each step that needs it, not once in V1 (question 8; section 7 says
how). Drafted on 2026-11-02 and revised the same day after an independent review and a
second reading (section 12 lists each finding and what changed for it). It is the first item of the parity matrix's "In short", the
largest gap a player meets, and the roadmap's next plan after phase P ("survival with the
rest of parity by the matrix's tiers, written and agreed with the owner before it is
built"). Its steps are V1 to V13; the names are new (`n10-brief.md`'s V1 to V3 were
subagents' batches inside N10 and are done).

Marks, as `n-plan.md` and `p-plan.md` use them:

- A `file:line` is Clustine's code at `3e1039e`, read there.
- `dir:Class.method @n` is the 26.3 jar's class as `javap -c -p` dumped it in
  `/home/claude/reference/<dir>-javap/`, at bytecode offset `n` (`n5:ServerPlayer.doTick
  @146` is `n5-javap/net.minecraft.server.level.ServerPlayer.txt`). `jar:Class.method @n`
  is a class with no dump, read from the jar
  (`target/datagen/26.3/versions/26.3/server-26.3.jar`) with the review's class-file
  disassembler (the session's `scratchpad/dis.py`, Python, no Java run), at offset `n`.
  Classes still to be read before their step are in section 9.
- `SteelMC` is the Rust port at `/home/claude/reference/SteelMC-26.3`, cited only where
  the class has not been read yet.
- `data/…` is the game's generated data in `target/datagen/26.3/generated/data/minecraft/`,
  `reports/…` its reports.
- **Inferred** is reasoning from what was read, to be tested; **Guess** is from memory of
  the game, checked against nothing here.

What a world keeps that V changes: a player's place grows (game mode, food, air, fire,
fall, absorption, the enchanting seed, the crafting grid and the cursor), each player gets
a book (recipe book, advancements, statistics), stacks keep the components a broken
container copies into its item, and furnaces, brewing stands and campfires keep their
contents in their chunks. **As the owner chose (question 8): each step defines the fields
it needs and raises `FORMAT_VERSION` (23) and the worker's `STATE_FORMAT` (31) itself**,
as every phase before did; a world made before a raise is refused, three or four times
in all over V (section 7).

---------------------------------------------------------------------------------------

## 0. What is proposed, in short (section 11 asks the owner)

1. **A player's game mode is their own**, kept in their place as the game keeps
   `playerGameType` and `previousPlayerGameType`. A new player gets **the server's default
   game mode**, set by `--gamemode` as the game's `server.properties` `gamemode` sets it,
   with `--force-gamemode` as the game's `force-gamemode`. V builds **survival and
   creative**; adventure and spectator are out of scope (question 11). The default stays
   `creative` until the survival core (V1 to V5) is in, then becomes `survival`, the
   game's default (question 1).
2. **Every "creative" assumption becomes a question asked of the player**: about a hundred
   and twenty places in the simulation and the edge (section 3.1) read the acting player's
   abilities (`has_infinite_materials`, `instabuild`, `invulnerable`, `may_fly`) or their
   attributes (reach), as the game does. The region refuses a creative-only input from a
   player who is not creative, as well as the edge, since an input may be on its way
   when the mode changes.
3. **Everything a player's body does is the region's, in the player's tick**: damage,
   knockback, hunger, air, fire, freezing, falling, regeneration, the breaking progress.
   It is deterministic (no clock; one stream of draws per player per tick, section 4.12),
   and durable as the place is now, in the `Has` note of every commit that changed the
   player (`api.rs:2336-2351`). **What moves with the player moves whole**: the breaking
   state, the crafting grid and the cursor go in `PlayerTransfer` and onto the survivor
   in a merge, as cooldowns and effects do (`state.rs:159-165`).
4. **What crosses a line goes by what exists, with what a player spends taken first.** A
   hit on a player another region holds is the entity effect `EntityUse::Hurt` sent to
   the player's holder (`entity_effects.rs:122-142`, already routed and refused there). A
   step across a line that uses up an item takes the item when the step is sent and gives
   back what the holder did not use (the result carried by `RemoteDone`, passed to the
   player's region by the edge as an input). A block broken across a line is a
   `RemoteStep` carrying the tool; its drops are the block's holder's, the tool's wear the
   player's region's on the answer (section 4.5, 4.6). Where regions follow players, a
   monster targeting a survival player across a line makes them merge; this is V4 work,
   since `target_at_line` passes over players today (`region/arrivals.rs:127-143`).
5. **Recipes, fuel, tool rules, hardness and enchantment effects come from the jar**
   (ADR-0019): recipes, advancements, enchantments, damage types and the cooking providers
   from the generated data, item components (`tool`, `food`, `cooking_fuel`,
   `equippable`, `enchantable`, `blocks_attacks`, …) from the reports, and block hardness
   and `requiresCorrectToolForDrops` from the code through `Extract.java`, as other block
   tables are (they are not in `reports/blocks.json`).
6. **Two milestones in one plan**: the survival core (V1 to V5: game mode, breaking,
   crafting, health with hunger, furnaces), after which a player can play the game's first
   hour; then the rest (V6 to V13: enchantments' effects, the enchanting table, the anvil
   and the grindstone, the recipe book, brewing, the other work blocks, the beacon,
   advancements, statistics). The owner tries it with real clients after each
   (question 9).

## 1. What a player meets when V is done

They join in survival with an empty inventory, in a world with trees and ores (question
14). They punch a tree, and the log comes off after the game's time with the cracks shown
to others; it drops, and they pick it up. They open their own inventory, make planks in the
2×2 grid and a crafting table, and on it a wooden pickaxe; stone breaks with it and drops
cobblestone, and the pickaxe wears and breaks. Night comes, and zombies and skeletons go
for them; a hit knocks them back; armour takes some of the damage. A creeper blows a hole
in the ground and in them. They fall too far and are hurt; they swim too long and drown;
they walk into lava, cactus, berry bushes, powder snow. They get hungry by running,
jumping, fighting and digging, eat, and heal when full. They die, and their inventory and
some experience lie where they died; the death screen shows the game's message, and they
respawn at their bed with a full bar and an empty inventory. They smelt iron and cook food
in a furnace, a smoker or a blast furnace, and the furnace pays experience when it is
emptied. Ores give experience when broken. A shulker box broken keeps its contents in the
item. Later: enchanted gear works; an enchanting table offers three enchantments for their
levels and lapis; an anvil repairs, combines and renames; a grindstone takes enchantments
off; the recipe book shows what they have unlocked and fills a grid; a brewing stand
brews; stonecutters, looms and smithing tables work; a beacon gives its effects;
advancements pop up as toasts and fill their screen; statistics count. The dragon now
hurts them, raiders attack them, and iron golems turn on a player villagers dislike.

### 1.1 What they do not see, named

- **No commands**: no `/gamemode`, `/give`, `/kill` in chat; how an operator changes a
  mode is question 2.
- **Adventure and spectator** (question 11).
- **Lightning** is not built (weather is not started): its damage and the Channeling
  enchantment wait for weather.
- **Maps** are not built, so a cartography table opens and holds items but makes
  nothing, and the explorer maps of trades stay absent (question 10).
- **Fishing** is not built: Luck of the Sea and Lure have nothing to act on, and the
  fishing advancements cannot be had.
- **Monster spawners and trial spawners** stay inert (matrix item 4).
- **Game rules** are the game's defaults and cannot be changed (question 4).
- **The world border** is not built (matrix T0), so its damage is not either.

## 2. What the game does

### 2.1 Game modes and the server's settings

- `n9:GameType <clinit>`: SURVIVAL 0, CREATIVE 1, ADVENTURE 2, SPECTATOR 3; `DEFAULT_MODE`
  is SURVIVAL. `isSurvival` is survival or adventure. `updatePlayerAbilities @0-91`:
  creative sets `mayfly`, `instabuild`, `invulnerable` (not `flying`); spectator those but
  `instabuild` and sets `flying`; survival and adventure clear all four; then every mode
  sets `mayBuild = !isBlockPlacingRestricted()` (adventure and spectator may not).
- `n5:ServerPlayerGameMode.changeGameModeForPlayer @0-109`: no change if equal; keeps the
  old as previous; lands a flyer near the ground; `onUpdateAbilities`; a
  `PlayerInfoUpdate(UPDATE_GAME_MODE)` to **every** player (`broadcastAll @70-84`);
  `updateSleepingPlayerList`. `n5:ServerPlayer.setGameMode @0-92` adds
  `GameEvent(CHANGE_GAME_MODE, id)` and a second abilities packet.
- **A new player's mode**: `ServerPlayer.<init> @251-268` calls
  `calculateGameModeForNewPlayer @0-61`: the server's forced mode if any, else the saved
  `playerGameType`, else `MinecraftServer.getDefaultGameType` (the world data's game
  type, `n9:MinecraftServer` line 1095). `jar:DedicatedServer.getForcedGameType` is the
  world's game type when `force-gamemode` is on, and `jar:DedicatedServer.initServer
  @151-160` writes the `gamemode` property into the world data at **every start**.
  `storeGameTypes @0-40` saves both modes; `restoreFrom @17-38` keeps them over a death.
- **The properties' defaults** (`jar:DedicatedServerProperties.<init>`): `gamemode`
  survival (@160), `force-gamemode` false (@89), `difficulty` easy (@129),
  `spawn-protection` 16 (@384). There is **no `pvp` property in 26.3**: PvP is the game
  rule `pvp` (`n5:ServerLevel` line 4429 @1-13, `GameRules.PVP`).
- **Spawn protection** applies only while the server has operators
  (`jar:DedicatedServer.isUnderSpawnProtection @21-34`); Clustine has none, so it has
  none, as the game's would not.
- **Flying**: `n5:ServerGamePacketListenerImpl.handlePlayerAbilities @13-44` sets
  `flying = packet.isFlying && mayfly`; nobody is kicked. `handleSetCreativeModeSlot @16`
  asks `hasInfiniteMaterials` and does nothing otherwise.
- **Reach**: `n6:Attributes.<clinit>` gives `block_interaction_range` 4.5 (@214) and
  `entity_interaction_range` 3.0 (@350); creative adds a modifier of 0.5 to the first and
  2.0 to the second (`n7:Player`, line 41); the reach is measured from the eyes to the
  block's or the entity's box, with the server's tolerance on top
  (`handleBlockBreakAction`'s `+1.0`).
- **Invulnerable until loaded**: `n5:ServerPlayer.isInvulnerableTo @27-33` refuses
  damage while `!connection.hasClientLoaded()`, which `PlayerLoaded` (serverbound) or a
  timeout sets, at a join, after a change of dimension, and after a respawn
  (`n5:ServerGamePacketListenerImpl.handleClientCommand @105`,
  `restartClientLoadTimerAfterRespawn`).

### 2.2 Damage to a player

`n7:LivingEntity.hurtServer`, in order: refused if `isInvulnerableTo` (@0) or dead (@11);
fire with fire resistance refused (@20); wakes a sleeper (@42); a shield's blocking
(`applyItemBlocking @0-267`: the `blocks_attacks` component, the angle to the source, the
`bypasses_shield` tag, disabling for `getSecondsToDisableBlocking`); freezing ×5 for
`freeze_hurts_extra_types` (@103); a helmet worn takes falling blocks' damage and ×0.75
(@129). Then the **cooldown** (@184-269, `damageCooldownTime`, 20 ticks): in its second
half (above 10), unless the type is in `bypasses_cooldown` (@196-203; the tag is empty in
26.3), only damage above the last hurt is taken, with no animation; otherwise the whole is
taken and the cooldown and `hurtTime` (10) set. Then the attacker is remembered (100 ticks
for a player), the `DamageEvent`, **`markHurt` (@348) unless `no_impact`**, knockback 0.4
unless `no_knockback`, the totem (`checkTotemDeathProtection`), and death (@393).
`markHurt` (`n7:Entity.markHurt @1`) sets `syncVelocity`, and the entity's tracker then
sends `SetEntityMotion` (clientbound 103) to its watchers **and to the player themselves**
(`n5:ServerEntity.sendChanges @960-993`, `sendToTrackingPlayersAndSelf`): it is the only way
a client learns it was knocked back. A player's own attack on a player sends it at once
instead (`n7:Player.causeExtraKnockback @124-166`: `SetEntityMotion` to the hit
`ServerPlayer`, then `syncVelocity` cleared and the old motion restored).

`n7:Player.hurtServer @0-123`: refused while `abilities.invulnerable` unless
`bypasses_invulnerability` (out of the world, `/kill`); **difficulty scaling** for a type
that scales (`DamageSource.scalesWithDifficulty @0-83`: `never`, `always`, or
`when_caused_by_living_non_player`): peaceful 0, easy `min(d/2 + 1, d)`, hard `d·3/2`.
`n5:ServerPlayer.hurtServer @0-98` refuses a player's hit (or their arrow's) when the
`pvp` rule is off or teams forbid it. `n5:ServerPlayer.isInvulnerableTo @0-41`: while
changing dimension (but an ender pearl's) and before the client has loaded.

`actuallyHurt` (`n7:Player.actuallyHurt @0-146`): **armour**
(`getDamageAfterArmorAbsorb @0-36`, unless `bypasses_armor`: `hurtArmor`, then
`n7:CombatRules.getDamageAfterAbsorb`: `t = 2 + toughness/4`,
`r = clamp(armour − d/t, armour·0.2, 20)`, `f = r/25`, Breach's `armor_effectiveness`
on the attacker's weapon, `d·(1 − f)`); **magic** (`getDamageAfterMagicAbsorb @0-233`:
Resistance `d·(25 − 5(amp+1))/25` unless `bypasses_resistance`, then the enchantments'
protection, `EnchantmentHelper.getDamageProtection` capped at 20, i.e. 80 %, unless
`bypasses_enchantments`); **absorption**; **if nothing is left, it returns here
(@90-95)**, before exhaustion and the combat tracker; otherwise
`causeFoodExhaustion(type.exhaustion)` (@96-101), the combat tracker, the health. Armour
wear: `n7:Player.hurtArmor` → `LivingEntity.doHurtEquipment @0-111`, `max(1, d/4)` to each
worn piece that can be hurt by the source.

The damage types are data (`data/damage_type`, 51) with their exhaustion (0.1 for most
blows, 0 for drowning, falling, starving, magic …) and scaling; the tags that act are
`bypasses_armor`, `bypasses_cooldown`, `bypasses_effects`, `bypasses_enchantments`,
`bypasses_invulnerability`, `bypasses_resistance`, `bypasses_shield`, `damages_helmet`,
`is_fall`, `is_fire`, `is_explosion`, `is_projectile`, `is_drowning`, `is_freezing`,
`no_impact`, `no_anger`, `no_knockback` (`data/tags/damage_type`).

### 2.3 Every source, where it comes from

| Source | Amount and rule | Where |
|---|---|---|
| Falling | `floor((dist + 1e-6 − safe_fall_distance) · mult · fall_damage_multiplier)`; none with `mayfly`; Jump Boost +1 safe block a level; halved fall distance in lava | `n7:LivingEntity.causeFallDamage @0-119`, `calculateFallDamage @0-35`, `n7:Player.causeFallDamage @0-41`, `n7k:MobEffects <clinit> @241-250`; the server's fall distance from the client's moves `n5:ServerPlayer.checkFallDamage` (line 3048) |
| Ender pearl | `ender_pearl` 5 to its thrower on landing | `ThrownEnderpearl.onHit`; refused today (`entity_effects.rs:145-148`) |
| Fire | 1 every 20 ticks while burning (`on_fire`); fire block `in_fire` 1 and ignition for 8 s; soul fire 2 | `n7:Entity.baseTick @126-187`, `n6:BaseFireBlock.entityInside`, `d:SoulFireBlock.<init> @2` |
| Lava | `lava` 4 and 15 s of fire | `Entity.lavaHurt`, `lavaIgnite` |
| Campfire, magma | `campfire` 1 (soul 2), `hot_floor` 1 unless sneaking | `n7:CampfireBlock.entityInside`; magma: SteelMC `magma_block.rs` |
| Drowning | air −1 a tick with eyes in water (Respiration may skip it: a draw), at −20 `drown` 2 and air 0; +4 a tick out of water; maximum 300 | `LivingEntity.baseTick @206-415`, `n7:Entity.TOTAL_AIR_SUPPLY`, `LivingEntity.getMaxAirSupply @0` |
| Suffocation | `in_wall` 1 a tick, held back by the cooldown | `baseTick @96-116` |
| Starving | section 2.5 | `jar:FoodData.tick` |
| Explosions | `((i² + i)/2 · 7 · 2r + 1)`, `i = (1 − dist/2r) · seen`; scaled `always` | `n6:ExplosionDamageCalculator.getEntityDamageAmount @62-63` |
| Projectiles | arrow `ceil(speed · 2.0)` plus enchantments; tridents, snowballs, fireballs, shulker bullets, spit … as built for mobs | `AbstractArrow.onHitEntity` and the projectiles of N9 |
| Mob melee | `attack_damage` plus the weapon, enchantments; scaled by difficulty | `n7:Mob.doHurtTarget @0-127` |
| The void | `fell_out_of_world` 4 below `minY − 64`, through invulnerability | `n7:LivingEntity.onBelowWorld`; built (`void.rs`) |
| Freezing | in powder snow +1 a tick to 140; full, every 40 ticks `freeze` 1; outside −2 a tick; leather worn prevents | `LivingEntity.aiStep @679-761`; SteelMC `entity/base/mod.rs:1520` |
| Magic | Poison `magic` 1 every `25 >> amp` while health > 1; Wither 1 every `40 >> amp`; Instant Damage `6 << amp` | `n9:PoisonMobEffect`, `d:WitherMobEffect`, `n9:HealOrHarmMobEffect` |
| Cactus | `cactus` 1 | SteelMC `cactus_block.rs` |
| Sweet berry bush | 1 when moving ≥ 0.003 and the bush is grown; slowed | `n7:SweetBerryBushBlock.entityInside` |
| Pointed dripstone | stalagmite: fall damage of `dist + 2.5`, ×2; stalactite: falling block | `d:PointedDripstoneBlock.fallOn`, `n6:FallingBlockEntity.causeFallDamage @0-207` |
| Falling anvils and blocks | `min(floor(n · perDistance), max)` to non-creative living entities | `n6:FallingBlockEntity.causeFallDamage` |
| Flying into a wall | `(old − new horizontal speed) · 10 − 3` | `LivingEntity.handleFallFlyingCollisions @0-55` |
| Cramming | 6 when more than `maxEntityCramming` (24) − 1 pushables, **on a draw of one in four** (`random.nextInt(4)`) | `LivingEntity.pushEntities @41-153`, the draw @74-78 |
| World border | `max(1, floor(−dist · damagePerBlock))` beyond its safe zone | `baseTick @119-201`; no world border is built |
| Lightning | `lightning_bolt` 5 and 8 s of fire | `Entity.thunderHit @0-36`; **not built** |
| The dragon | wings push and hit `mob_attack` 5, head 10, only on players that are not creative or spectators | `d:EnderDragon.aiStep @1265-1391`, `knockBack @0-227`, `hurt @0-71` |

### 2.4 Death and the respawn

- `n5:ServerPlayer.die @0-365` (not `LivingEntity.die`): the `PlayerCombatKill` with the
  combat tracker's message, broadcast by the team's visibility; neutral mobs forgive;
  **`dropAllDeathLoot` unless a spectator**; the kill's statistics and scores; fire and
  freezing cleared; `lastDeathLocation`.
- `n7:Player.dropEquipment @0-35`: unless `keepInventory`, items with Curse of Vanishing
  (`prevent_equipment_drop`) are destroyed and `Inventory.dropAll` throws the rest.
  `n7:Player.getBaseExperienceReward @0-40`: `min(level · 7, 100)` points as orbs, none
  with `keepInventory` or for a spectator (@16-26).
- `n5:ServerPlayer.restoreFrom @0-335` after a death: full health, a new FoodData (food
  20, saturation 5), effects cleared, inventory and experience gone (kept only with
  `keepInventory`); kept are the game modes, the enchanting seed, the ender chest, the
  recipe book, `seenCredits`, the last death. The respawn itself is built
  (`p-respawn-facts.md`, `respawning.rs`).

### 2.5 Hunger, regeneration, eating

`jar:FoodData`: food 20, saturation 5 at most the food, exhaustion to 40. Its `tick`,
called from `n5:ServerPlayer.doTick @42`:

- @11-75: exhaustion above 4 loses 4, and takes 1 of saturation if there is any, else 1
  of food (not in peaceful).
- **One timer** (`tickTimer`) serves the three branches below, counted up by one a tick in
  whichever applies, reset to 0 by a heal or a starve and by the last branch.
- **fast** (@79-176): with `naturalRegeneration`, saturation above 0, hurt and **food at
  20**, at 10 ticks heal `min(saturation, 6)/6` and add `min(saturation, 6)` exhaustion.
- **slow** (@181-235): hurt and food at least 18, at 80 ticks heal 1 and add 6 exhaustion.
- **starving** (@239-311): food 0, at 80 ticks `starve` 1 while health is above 10 (easy),
  above 1 (normal), always (hard).
- otherwise (@319-320) the timer is reset.

Exhaustion (`n7:Player.causeFoodExhaustion @0-29`, none while invulnerable): a jump 0.05,
a sprinting jump 0.2 (`n5:ServerPlayer.jumpFromGround @19, @29`); 0.01 a metre swimming or
walking in or on water, 0.1 a metre sprinting, none walking
(`checkMovementStatistics @66-295`); an attack 0.1 (`n7:Player.attack @334`); a block
broken with the correct tool 0.005 (`n6:Block.playerDestroy @12`, 2.6); a hurt its type's
exhaustion. **Peaceful** (`jar:ServerPlayer.tickRegeneration @0-124`, with
`naturalRegeneration`): every 20 ticks heal 1 and raise saturation by 1 (@39-85), every 10
ticks food +1 if needed (@92-121). `doTick @146-264` sends `SetHealth(health, food,
saturation)` when the health or food changed or the saturation became or stopped being
zero.

Eating: `n9:Consumable` (1.6 s by default, `startConsuming` refused when `canConsume`
fails), `n7:Player.canEat` (invulnerable, `can_always_eat`, or food below 20),
`n9:FoodProperties.onConsume` → **`jar:FoodData.eat(FoodProperties) @0-12`: food `+
nutrition` to 20, saturation `+` the component's `saturation` as it stands, to the food**
(`add @0-35`); the component's saturation is already `nutrition · modifier · 2`, made by
its builder, and Clustine's `Food.saturation` is that final value
(`clustine-data/src/lib.rs:587-593`). `eat(int, float)`, which multiplies, is the
Saturation effect's.

### 2.6 Breaking

`n5:ServerPlayerGameMode.handleBlockBreakAction @0-700`:

- START: beyond reach (+1) aborts; spawn protection (none here, 2.1) and `mayInteract`;
  `instabuild` destroys at once (creative, built); adventure's restriction; otherwise
  `destroyProgressStart = gameTicks`, `EnchantmentHelper.onHitBlock`, **`state.attack`**
  (a note block plays, a redstone ore lights, a dragon egg teleports), `p =
  getDestroyProgress`; `p ≥ 1` breaks at once; else `isDestroyingBlock` with
  `destroyBlockProgress(id, pos, p·10)`.
- **CHANGE_DESTROY_DIRECTION** (@515-526, new in 26.x): updates the face being broken.
- STOP (@529): if the position is `destroyPos`, `p · (gameTicks − start + 1) ≥ 0.7`
  breaks; else a **delayed destroy** that the tick finishes when the progress reaches 1
  (`tick @0-197`). **A STOP at another position only logs** (@538-544 → @672): nothing is
  broken.
- ABORT: progress −1 to others. `tick` sends `BlockDestruction` when `(int)(p·10)` changes
  and the sound every 4 ticks; `n5:ServerLevel.destroyBlockProgress` sends it to the
  level's other players within 32 blocks. **The progress is recomputed each tick as the
  destroy speed now times the ticks since the start plus one**
  (`incrementDestroyProgress @0-65`), not accumulated: a change of tool or of Haste in
  mid-dig acts on the whole dig.
- The acknowledgement: `n5:ServerGamePacketListenerImpl.handlePlayerAction @299-307`
  calls `handleBlockBreakAction`, **then** `ackBlockChangesUpTo(sequence)`; the client
  keeps its predicted air until the acknowledgement, and the block change sent before it
  is what it then shows.
- `destroyBlock @0-273`: `canDestroyBlock`; `playerWillDestroy`; remove; unless creative,
  `hasCorrectToolForDrops`, `ItemStack.mineBlock` (wear, `n5:Item.mineBlock`: the tool's
  `damage_per_block` when hardness is not 0), and, **only if removed and with the correct
  tool** (@246-269), `Block.playerDestroy` (the mined statistic, the 0.005 exhaustion,
  `dropResources` with the tool, `spawnAfterBreak`: ores' experience,
  `Block.tryDropExperience @0-31`, 0 with Silk Touch).
- `n7:BlockBehaviour.getDestroyProgress @0-49`: hardness −1 never; else
  `speed / hardness / (correct ? 30 : 100)`. `n7:Player.getDestroySpeed @0-140`: the
  tool's speed (`tool` component rules, 1 without), `+ mining_efficiency` if above 1
  (Efficiency `level² + 1`, `data/enchantment/efficiency.json`), Haste or Conduit Power
  `×(1 + 0.2(amp+1))`, Mining Fatigue `×0.3^(amp+1)`, `×block_break_speed`,
  `×submerged_mining_speed` with eyes in water, `/5` off the ground.
  `n7:Player.hasCorrectToolForDrops`: the block does not require one, or the tool's
  rules say it is correct.
- **What the item keeps of the block**: a block's loot table may `copy_components` from
  its block entity into the dropped item. Shulker boxes copy `custom_name`, `container`,
  `lock`, `container_loot` (`data/loot_table/blocks/*_shulker_box.json`); banners their
  patterns, beehives and nests their bees, decorated pots their sherds, named containers
  their name. A shulker box broken by a survival player keeps its contents **only** in
  the item.
- Data: 38 items carry `tool` with rules over `#mineable/*`, `#incorrect_for_*_tool`,
  `#sword_efficient` and others (`reports/minecraft/components/item`); hardness and
  `requiresCorrectToolForDrops` are only in code (`n6:BlockBehaviour$Properties` lines
  13-15, 403-594).

### 2.7 Wear

`n7:ItemStack.hurtAndBreak @0-54` and `processDurabilityChange @0-34` (none for infinite
materials; Unbreaking's `remove_binomial`: armour `2L/(10 + 5(L−1))`, other items
`L/(L+1)`); breaking broadcasts the equipment-break entity event. Weapons wear on a hit
(`hurtEnemy`), tools on a break, armour on a hurt, bows, shears, flint and steel, fishing
rods and others on use.

### 2.8 Experience

Orbs, levels and their pick-up are built (`experience.rs`, `orbs.rs`). Survival adds:
from ores (2.6), from furnaces (2.10), lost at death (2.4), and spent at the enchanting
table and the anvil. Mending (`repair_with_xp`) is ported but unused
(`items/enchantments.rs:273-285`).

### 2.9 The inventory screen, crafting and the recipe book

- `n6:InventoryMenu`: container 0, result 0, grid 1-4, armour 5-8 (head first), main
  9-35, hotbar 36-44, off hand 45; `slotsChanged` crafts as the crafting table does;
  `removed` returns the grid to the inventory (`AbstractContainerMenu.clearContainer`
  line 1866, `dropOrPlaceInInventory` line 1826: thrown only if there is no room or the
  player leaves). The inventory menu is the player's for their whole life; the client
  sends `ContainerClose(0)` when it shuts its screen, which empties the grid
  (`n5:ServerGamePacketListenerImpl.handleContainerClose`, line 13691 of the verbose
  dump). The cursor's stack belongs to the open menu (`carried`).
- `jar:CraftingMenu.slotChangedCraftingGrid`: `RecipeManager.getRecipeFor(CRAFTING)`,
  `setRecipeUsed` (refused under `limitedCrafting` for a recipe not in the book), the
  result to slot 0 with its own `ContainerSetSlot`. **The client has no recipes in 26.3
  and cannot predict a result**: it waits for that packet. `n6:ResultSlot.onTake` (line
  170) takes one of each ingredient and leaves the remainders (buckets, bottles);
  `checkTakeAchievements` (line 87) awards the crafted statistic and the recipe.
- Recipes (`data/recipe`, 2,042): 827 shaped, 375 shapeless, 351 stonecutting, 279
  brewing, 73 smelting, 33 transmute, 25 blasting, 18 smithing trim, 16 banner
  duplicate, 12 smithing transform, 9 smoking, 9 campfire, 6 dye, and one each of
  imbue, decorated pot, book cloning, firework rocket, star and fade, map extending,
  repair and shield decoration.
- **26.3's recipe packets**: `UpdateRecipes` (clientbound 136) carries only the property
  sets and the stonecutter's recipes, not the recipes; the book is
  `RecipeBookAdd`/`Remove`/`Settings` (75, 76, 77) with each entry's display;
  `PlaceRecipe` (serverbound 39) asks the server to fill a grid, answered by
  `PlaceGhostRecipe` (64) where it cannot; `RecipeBookChangeSettings` (47) and
  `RecipeBookSeenRecipe` (48) (`jar:ServerRecipeBook`).
- **Unlocking is by advancements**: 1,739 of `data/advancement/recipes` reward one
  recipe each, when an item enters the inventory (`inventory_changed`) or another
  recipe is unlocked (`recipe_unlocked`); 303 recipes are never in the book (brewing and
  the special ones). Without the book a player can still craft by hand (`limitedCrafting`
  off by default).

### 2.10 Furnaces, smokers, blast furnaces, campfires

- `jar:AbstractFurnaceBlockEntity`: input 0, fuel 1, result 2; saved as
  `cooking_time_spent`, `cooking_total_time`, `lit_time_remaining`, `lit_total_time`,
  `speed_multiplier`, the items, and `RecipesUsed` (`saveAdditional`). The fuel is the
  item's `cooking_fuel` component (347 items) with its burn time from
  `context_int_provider/cooking/time_*` (coal 1600) and its own `speed_multiplier`. The
  smoker and the blast furnace cook twice as fast (`context_float_provider/cooking/
  speed_default.json`, the `fast_cooking` block predicate) **and burn their fuel in half
  the time** (the providers divide by `fast_burn_time_reduction_factor`, 2). Cooking takes
  the recipe's 200 ticks over the speed; unlit, the timer falls by 2; **a fresh fuel
  rescales a cook in progress to `ceil(timer / total · newTotal)`** (`serverTick
  @244-298`). The `lit` block state follows. `jar:AbstractFurnaceMenu`: four data values
  (`ContainerSetData`, clientbound 19). Hoppers: input from above, fuel from the sides,
  result and empty buckets from below.
- **Experience**: `recipesUsed` is a hash map of recipe to count; on taking the output
  (`FurnaceResultSlot.checkTakeAchievements`) and on breaking the furnace,
  `awardUsedRecipesAndPopExperience` pays each recipe's experience times its count, the
  fraction by a draw from **the level's random** (`createExperience @26`). Clustine walks
  the map in a fixed order (by recipe key) and draws from the region's stream; the
  official server's order is its hash map's, which only the split into orbs shows
  (named).
- `jar:CampfireBlockEntity`: four slots, `CookingTimes` and `CookingTotalTimes`, 600 ticks
  each (`campfire_cooking`), drops the result, no experience, cools by 2 when unlit.
  Clustine's campfires burn but do not cook (matrix T1).

### 2.11 Brewing

`jar:BrewingStandBlockEntity`: bottles 0-2, ingredient 3, fuel 4; 400 ticks a brew; blaze
powder's `brewing_fuel` gives 20 uses; saved with brew time, fuel, their totals and a speed
(`saveAdditional`). In 26.3 brewing is a **recipe type** (279 files in
`data/recipe/brewing`) and `PotionBrewing` is gone. `jar:BrewingStandMenu`.

### 2.12 The enchanting table, the anvil, the grindstone

- `jar:EnchantmentMenu`: item 0, lapis 1; ten data values (three costs, the seed, three
  clues and three levels); bookshelves around (`enchantment_power_provider`, through
  `enchantment_power_transmitter`) to 15; `n9:EnchantmentHelper.getEnchantmentCost`
  (line 1061) and `selectEnchantment` (line 1171) from a random seeded by the player's
  **enchantment seed** (`XpSeed`, re-rolled at each enchanting,
  `ServerPlayer.onEnchantmentPerformed` line 847); a button (`ContainerButtonClick`,
  serverbound 17) costs `id + 1` lapis and levels.
- `jar:AnvilMenu.createResult`: repairs with the material or a second item (+12 %),
  merges enchantments (`anvil_cost × level`, halved from a book), renames
  (`RenameItem`, serverbound 49, cost 1), the prior work penalty, "Too Expensive" at 40;
  `onTake` takes the levels and, outside creative, chips the anvil with chance 0.12.
- `jar:GrindstoneMenu`: removes all but curses, merges with +5 %, pays back experience
  from the enchantments' minimum costs.

### 2.13 The other work blocks

- `jar:StonecutterMenu`: input, result, one data value; the button is the index into the
  351 recipes the client has from `UpdateRecipes`.
- `jar:LoomMenu`: banner, dye, pattern, result; 32 patterns need no item, 10 items
  provide the others.
- `jar:SmithingMenu`: template, base, addition; 12 netherite upgrades, 18 trims over 11
  materials.
- `jar:CartographyTableMenu`: needs a filled map with its data: no maps exist.
- `jar:BeaconMenu`: one payment, three data values, `SetBeacon` (serverbound 53).

### 2.14 The enchantments' effects

43 enchantments in `data/enchantment`, each a list of data-driven effects (31 kinds of
`enchantment_effect_component_type`). What survival needs that is not built (the matrix,
T1): Protection, Fire, Blast and Projectile Protection and Feather Falling
(`damage_protection`); Thorns (`post_attack`); Sharpness, Smite, Bane of Arthropods,
Impaling (`damage`); Fire Aspect, Knockback, Wind Burst; Efficiency, Aqua Affinity,
Respiration, Depth Strider, Swift Sneak, Soul Speed, Sweeping Edge (`attributes`); Silk
Touch (`block_experience` and loot predicates), Fortune and Looting (loot's
`apply_bonus`, `match_tool`, `equipment_drops`); Frost Walker; Breach and Density; the
curses (Binding `prevent_armor_change`, Vanishing `prevent_equipment_drop`); Mending's
use. Luck of the Sea, Lure wait for fishing, Channeling for lightning. Entry points:
`n9:EnchantmentHelper.processDurabilityChange` (line 115), `getDamageProtection` (333),
`modifyDamage` (351), `doPostAttackEffects` (427). Villagers already sell enchanted
tools (N10), so Efficiency, Silk Touch, Fortune and Unbreaking (built) on a tool must act
from the first survival dig: they come with breaking (V2), the protections with damage
(V4).

### 2.15 Mobs and players

`n7:NearestAttackableTargetGoal.findTarget @0-60` finds the nearest player passing
`n7:TargetingConditions.test`, which asks `canBeSeenAsEnemy`; `n7:Player.canBeSeenAsEnemy`
is false while `abilities.invulnerable` (creative and spectator), and `LivingEntity.canAttack`
refuses players in peaceful. `n6:EntitySelector.NO_CREATIVE_OR_SPECTATOR` filters the
hurt-by and neutral anger, the dragon's wings and head, falling blocks. A zombie has
the player at priority 2 (`n7:monster.zombie.Zombie.addBehaviourGoals`). Every hostile
mob Clustine has (section 3.1) would then chase, attack and shoot a survival player.
Monsters near a bed keep a survival player awake (`Player.BedSleepingProblem.NOT_SAFE`;
`ServerPlayer.startSleepInBed`, line 2727, to be read for the box).

### 2.16 Advancements and statistics

- 1,866 advancements: 1,740 recipe ones (no display) and 126 shown (adventure 47,
  husbandry 31, nether 23, story 16, end 9; 91 tasks, 25 challenges, 10 goals; 23 give
  experience). `UpdateAdvancements` (clientbound 133) with the reset, the added, the
  removed and the progress, **each criterion with the date it was obtained** (an
  `Instant` of the wall clock, which the advancement screen shows); `jar:PlayerAdvancements`
  awards **in every game mode**. Triggers across the shown ones: `player_killed_entity`
  89, `location` 67, `consume_item` 41, `inventory_changed` 28, `recipe_crafted` 27,
  `bred_animals` 27, `tame_animal` 21, `placed_block` 15 and 40 more with one to ten uses
  (58 trigger types in the registry). The recipe ones use `inventory_changed` and
  `recipe_unlocked` (and `enter_block` 11, `tick` and `impossible` once).
- Statistics: `jar:ServerStatsCounter`; `AwardStats` (clientbound 3) only when the client
  asks (`ClientCommand` REQUEST_STATS, serverbound 12); nine kinds (mined, crafted, used,
  broken, picked up, dropped, killed, killed by, custom) and 78 custom ones, some of
  which change every tick (`play_time`, `time_since_rest`, `*_one_cm`;
  `n5:ServerPlayer.doTick @50-102`, `checkMovementStatistics`).

### 2.17 What waits for survival elsewhere

- **The dragon** hits and throws players that are not creative (2.3); the fight is built
  (`ender_dragon.rs:428` filters creative players today).
- **Raids**: a captain's loot (`entities/pillager` with `raider.is_captain`) drops an
  ominous bottle, drunk for Bad Omen; `n10:BadOmenMobEffect.applyEffectTick` turns it into
  the Raid Omen in a village (built: `Omens`, `api.rs:286-292`). What survival changes is
  that raiders attack the player and the player can die (the raid's bar counts only
  living heroes, `raids.rs:534`).
- **Villagers' reputation**: `n7:Villager.onReputationEventFrom @0-141` is built for
  players' hits; survival adds iron golems attacking a survival player whose reputation
  is low (`iron_golem.rs:479, 668`).

## 3. What Clustine has

### 3.1 The game mode, and what assumes creative

There is no game mode on any record. It is told, hard-coded, in four places:
`PlayerFace::of` (`api.rs:320-321`, `game_mode: 1`) and `WorldPlayer::game_mode`
(`api.rs:397`), which mobs read (`region/mobs.rs:777-778`); the edge's
`CREATIVE_ABILITIES` (`fanout.rs:66-67`) sent at a join, a placing and a respawn
(`fanout.rs:3799-3856, 3950-3954, 5684-5701`); the player list (`fanout.rs:7438`); the
`Login` and `Respawn`'s `spawn_info` (`fanout.rs:7961-7962`). The edge accepts
`SetCreativeModeSlot` from anyone (`play.rs:282-293`) and the client's flying
(`play.rs:295-300`).

What assumes creative, by subsystem (each to read the acting player's abilities or
attributes):

- **Reach**: `BLOCK_REACH 6.0` (`region.rs:401-403`); `CREATIVE_ENTITY_REACH` 8
  (`uses.rs:13-15`, used by `uses.rs:67`, `picks.rs:44`, `granted.rs:129`); trading's
  range (`trading.rs:37`).
- **Breaking**: the edge turns `START_DESTROY_BLOCK` into `PlayerInput::Dig` and
  acknowledges ABORT and STOP at once (`play.rs:451-466`); `break_by_player` makes no
  drops (`updates.rs:357-417`); ice leaves air (`ice.rs:93-108`); a punch never calls the
  block's `attack` (a note block played by a punch, `instruments.rs:13-17`); hives give
  no bees' anger and no drop (`hives.rs:19`); `cushions.rs:180`.
- **Things a player breaks at once with no drop**: armour stands, minecarts, boats,
  hanging entities, knots, the vehicles (`stands.rs:804-826`, `vehicles.rs:149-215`,
  `minecarts.rs:1402-1427`, `boats.rs:1444-1471`, `hanging.rs:668-853`, `knots.rs:96`,
  `attached.rs:28-39`), because every attack and interact is sent `creative: true`
  (`region.rs:2777-2793, 2814`; the bit is in the format,
  `clustine-format/src/effects.rs:13, 356, 391`); a player's attack is sent with
  `sprinting: false` (`region.rs:2788`), so no sprint knockback.
- **Pick-block**: the creative path only (`picks.rs:29`).
- **Items not used up**: `has_infinite_materials()` is `true` (`anchors.rs:23-26`,
  `throws.rs:46-47`), and some thirty places keep the item (ammunition, buckets, eggs,
  bone meal, eyes of ender, flint and steel, the mobs' feeding and taming in
  `goat.rs`, `equine.rs` and others); a placed block is never taken from the stack
  (`Click.held` carries only the item's id, `region.rs:3003`); `eaten()` is dead code
  (`items/consumables.rs:193-210`); weapons do not wear (`mob_attacks.rs:13`).
- **Pick-up**: a creative player takes the whole stack and the rest is destroyed
  (`items.rs:711-767`); creative-only arrows (`arrows.rs:1339-1376`,
  `projectiles.rs:148-156`, `bow.rs:157-165`).
- **Menus**: `menus::click(…, true)` (`menu_play.rs:743-744`): clone, clone drag, overflow
  deleted (`menus.rs:258, 303, 983-1011`).
- **Invulnerability**: a hurt on a player is refused (`entity_effects.rs:85-120`, and in
  `strike` at 122-142), instant health and harming skipped on players
  (`entity_effects.rs:96-99, 376`), a pearl's 5 refused (`entity_effects.rs:145-148`),
  Regeneration and Hunger do nothing on players (`player_effects.rs:1-18, 237`), arrows
  refused (`arrows.rs:1081-1083`), and a dozen projectiles and mob attacks skip players
  (`thrown.rs`, `shulker_bullet.rs`, `fireworks.rs`, `spit.rs`, `fangs.rs`,
  `guardians.rs:451`, `ravager.rs:17`, `hoglin_ai.rs:516`, `magma_cube.rs:28`,
  `pufferfish.rs:12`, `mobs/tick.rs:126`); explosions only push
  (`explosions.rs:522-526`); a portal fires at once (`portals.rs:34, 470-475`); TNT's
  owner is creative (`explosions.rs:146-149`, the format's `BLAST_BY_CREATIVE`); no
  invulnerability while the client loads (`PlayerLoaded` was dropped, `play.rs:327-329`; an input since 825e62e,
  `region/loading.rs`).
- **Mobs' targeting**: `seen_as_enemy` is `!creative && seen_by_anyone()`
  (`mobs/ai/sensing.rs:136-138`) and the `NO_CREATIVE_OR_SPECTATOR` selectors
  (`goals.rs:729, 1742, 1755`, `neutral.rs:57, 112`, `fox.rs`, `iron_golem.rs`,
  `phantom.rs:407`, `elder_guardian.rs:50`, `ender_dragon.rs:428`, `brain/sensors.rs:210`).
  These read the face's mode and are right once the mode is. **Not so** the merge for a
  fight at a line: `target_at_line` passes over a player's face
  (`region/arrivals.rs:127-143`, "a face of a player is no mob's target, every player
  being creative").
- **Sleep**: monsters keep nobody awake (`sleep.rs:24-25`).
- **Raids**: players are taken to be alive for the bar's heroes (`raids.rs:534`).
- **The start**: every new player is given a hotbar of nine blocks
  (`bin/clustine/src/lib.rs:45-62`).

### 3.2 What of survival already exists underneath

- **The player's record**, `Place` (`api.rs:90-131`): inventory (41 slots), ender chest,
  experience, respawn point, last death, effects (with the ticks they have left), time
  since rest, raid omen, dimension, travel, credits, portal cooldown, **health** and
  **dead** (since format 21). Logged as `LoggedPlace` (`clustine-format/src/log.rs:267-305`,
  `put_place` 1267). The region's `PlayerState` (`state.rs:98-184`) adds the use in
  progress, cooldowns, attack strength, the open menu with its cursor (`state.rs:140-142,
  200-215`), which a hand-over closes and which is in no transfer. **Not kept**: food,
  saturation, exhaustion, air, absorption, fire, fall distance (only in memory for
  crits, `vibrations.rs:252-263`), freezing.
- **Damage**: the mobs' whole path (`mobs/body/hurt.rs`: types at 27-58,
  `actually_hurt` 405-449 with `CombatRules` armour and toughness at 453-457,
  resistance, absorption, cooldown, knockback 475, death and loot, experience
  569-612). Seventeen damage types named; no enchantment protection. No motion is ever
  sent to a player for knockback (only the explosion packet's push,
  `explosions.rs:519-526`).
- **The void's death** (`void.rs`): 4 a tick with the cooldown, 60 ticks of protection
  after a respawn, the `DamageEvent`, the death (dismounts, wakes, closes the menu, clears
  effects, the last death), then the **inventory and experience emptied, not dropped**
  (`void.rs:175-177`, "below the world, where nothing stays"), the entity events, the
  death message (`Durable::Died`, `api.rs:1305`; the edge's `PlayerCombatKill`,
  `fanout.rs:3737-3760`), every input but the respawn dropped while dead (221-237). The
  respawn at a bed, an anchor or the world spawn (`respawning.rs`, `respawn_block.rs`),
  with health and experience told (`arriving.rs:640-653`). Compared with the official
  server (`end_oracle.rs`).
- **Effects** (`mobs/body/effects.rs`): poison, wither, regeneration, instant health and
  damage, absorption, resistance, the attribute effects (Haste, Mining Fatigue,
  Strength …); on players they are kept, sent and their attributes told, but heal, hurt
  and exhaustion do nothing (`player_effects.rs:1-18, 237`).
- **Experience**: levels as the game reckons them (`experience.rs:22-60`), orbs and their
  pick-up (`orbs.rs:577-630`), from mobs a player killed (`hurt.rs:569-612`), Mending
  ported and unused.
- **Eating and using**: the use over time (`using.rs:104-260`, from `consume_seconds`),
  every consume effect (`eating.rs:120-165`), bows, crossbows, tridents; the `food`
  component is read (`clustine-data` `Food`, its saturation final), but nothing is fed
  and `canEat` is not asked.
- **A player's hit on a mob** (`mob_attacks.rs:76-140`): attack strength, crits from the
  fall, sweep; no enchantment (`enchanted = 0`, 107-108).
- **Loot** (`crates/clustine-blocks/src/loot.rs`): every block table with its conditions,
  `match_tool` always false and Fortune at 0 (462-464), explosion decay and survival real;
  **`copy_components` is ignored** (27-30, 367), and shulker boxes do not spill their
  contents (`containers.rs:95`), so a shulker box broken in survival would lose them.
  Stacks drop the components `custom_name`, `container`, block entity data, `bees`
  (matrix T1). Smelting recipes are read only for loot's `furnace_smelt`
  (`tools/datagen/src/loot.rs:501-510`).
- **Menus** (`crates/clustine-sim/src/menus.rs`, the edge's synchroniser
  `services/edge/src/menus.rs`): the game's clicks and quick moves for seven kinds
  (`api.rs:1034-1054`). Window 0, the player's own, is not handled
  (`edge/menus.rs:213-217`); the 2×2 grid is the client's own (`play.rs:284`). Furnaces,
  brewing stands and the rest are placed with no contents kept (`dispensers.rs:32-34`).
- **Data**: the tags `mineable/*`, `needs_*_tool`, `incorrect_for_*` are generated
  (`generated/tags.rs:1442-1744`) and unused; no `tool` component, no hardness, no
  `requiresCorrectToolForDrops` (`BlockInfo`, `clustine-data/src/lib.rs:350-368`).
- **Packets**: ids only for recipes, advancements and statistics
  (`clustine-protocol/src/generated/packet_ids.rs`); everything the edge does not know is
  `Unhandled` (`play.rs:331-333`), among them `ContainerButtonClick`, `PlaceRecipe`,
  `RenameItem`, `SetBeacon`, the recipe book and `SeenAdvancements`; a statistics request
  is ignored (`play.rs:307-311`).
- **Difficulty** is fixed: `Difficulty::WORLD = Easy` (`clustine-world/src/difficulty.rs:19`),
  read in 31 places, and its record says another difficulty needs the enchantment
  providers of mobs' equipment first (`difficulty.rs:1-6`).
- **Tests**: the official server is made creative by `server.properties`
  (`oracle.rs:37`, and `n9_oracle.rs:65`, `end_oracle.rs:78`, …); its console takes
  commands (`Oracle::command`, `oracle.rs:225-235`). The bots get items from the
  creative tab (`bot.rs:1216-1260`), dig with one START (`bot.rs:1086-1099`), fly; `join.rs:49`
  asserts creative.

### 3.3 Across lines, what exists

- A mob's hit on a player another region holds is sent to that player's holder as
  `EntityUse::Hurt` (`entity_effects.rs:136-139`) and refused there; projectiles and
  explosions reach entities across a line as entity effects (ADR-0028 section 3.5).
- A player's break or placing across a line is a `RemoteAction`/`RemoteStep`
  (`api.rs:1097-1182`), done by the holder (or passed on) and answered by
  `Durable::RemoteDone` to the **edge** (`api.rs:1215-1228`), which carries the blocks
  to show and nothing of the item.
- A player's whole state crosses in `PlayerTransfer` (`api.rs:438-505`) with the place;
  the edge passes on every input numbered above `last_input` to the new region
  (`api.rs:1189-1193`), so an input is never lost to a hand-over or a resume.
- Items in another region's land cross as arrivals and can be picked up only once they
  or the player have crossed (`items.rs:690-767`, ADR-0028 section 3.5).

## 4. The design

### 4.1 The game mode

- `Place` gains `game_mode: GameMode` and `previous_game_mode: Option<GameMode>`;
  `GameMode` is one type in `clustine-world` with the game's ids, and its `abilities()`
  is `updatePlayerAbilities` (`invulnerable`, `may_fly`, `instabuild`, `may_build`).
  `PlayerFace` and `WorldPlayer` carry the player's own mode instead of 1, so mobs,
  sleep's count and the store's view are right with no other change.
- **Scope**: `--gamemode` accepts `survival` and `creative`; `adventure` and `spectator`
  are refused at the start until built (question 11), since each needs more than
  abilities (spectators unseen, through blocks, the camera; adventure's
  `can_break`/`can_place_on`).
- The **default** is the world's: `--gamemode` on `clustine` (and on the world store's
  subcommand, where a new player is first placed) sets it. *As built (V1, 2026-11-03)*:
  it is the world store's setting, as `server.properties` is the game's server's, and
  said to every region the store opens (`Restored::game_mode`), not kept in `meta`: the
  game writes the property into its world data at every start
  (`DedicatedServer.initServer @151-160`), which nothing of Clustine's reads back. A player without a place gets it at the home
  region's entering (`StayNote::Entering`, ADR-0020). `--force-gamemode` sets every
  entering player to the default, as `calculateGameModeForNewPlayer` does.
- A change of mode (by `--force-gamemode` at a join, or the operator's tool of question
  2, whose `gamemode` and `give` reach the player's region as **a new kind of region
  input**, an operator's order, not a `PlayerInput`, since no client sent it) is the
  region's: it sets the mode and the previous one, lands a flyer as
  `changeGameModeForPlayer` does, and tells the edge, which sends the `GameEvent`, the
  abilities twice and the player list's update **to every player of every edge** (the
  player list is the edges'; a change is a `PlayerEvent` that the edge passes on as it
  passes a join). The edge's `spawn_info`, abilities and list entry take the player's
  mode from the region's report instead of the constant.
- **Refusals, at the edge and in the region**: `SetCreativeModeSlot` from a player who is
  not creative does nothing (`handleSetCreativeModeSlot @16`); the client's flying is
  `is_flying && may_fly`, never a kick (`handlePlayerAbilities @13-44`). The region asks
  again when the input arrives, since a mode may change between the edge's look and the
  region's tick.
- **Reach** from the player's attributes, as the game reckons it with its tolerance, in
  place of the creative constants of 3.1.
- **Invulnerable until loaded**: `PlayerLoaded` becomes an input (built in 825e62e),
  and the region refuses damage to a player who has not sent it since their join, their
  last change of dimension or their last respawn, as `isInvulnerableTo @27-33`, with the
  game's timeout.
- **The start**: a new survival player starts empty; the starting hotbar
  (`bin/clustine/src/lib.rs:45-62`) is given to creative players only, and made settable
  for the tests' kits (section 6).

### 4.2 The player's record

**What the tick reads every tick** goes in `Place`, as health did: the game mode and the
previous one, `food`, `saturation`, `exhaustion`, the food timer, `air`, `fire`,
`ticks_frozen`, `fall_distance`, `absorption`, the hurt cooldown and `last_hurt`, the
enchantment seed, the 2×2 grid and the cursor (4.7). These are what the game saves
(`Player.addAdditionalSaveData`: `XpSeed`, the abilities; `LivingEntity`'s
`AbsorptionAmount`, `HurtTime`, `Health`; `Entity`'s `Air`, `Fire`, `TicksFrozen`,
`fall_distance`, `invulnerable_time`; `FoodData`'s `foodLevel`, `foodTickTimer`,
`foodSaturationLevel`, `foodExhaustionLevel`).

**Counters are what is left or has passed**, never a tick number, since a place moves
between regions whose tick numbers differ: the food timer as the ticks counted, the hurt
cooldown, fire and the respawn's protection as the ticks left, as the place keeps
effects, the portal cooldown and the time since rest (`api.rs:111-121`). In the region
they are kept against its own ticks in `PlayerState` (as effects' ends are,
`state.rs:162-165`), so that a state changes only when something happens, and turned into
what is left whenever the place is written: in every `Has` the player's actions make, at
the worker's checkpoints (as resting entities are saved, `api.rs:2366-2370`), and in a
`PlayerTransfer`. A merge moves them onto the survivor's ticks, as cooldowns. **Named**:
after a crash such a counter comes back as of the last commit that wrote the place, so a
fire burns a little longer and the food timer counts again from where it was; nothing a
player was shown changes but a few ticks of these counters (health, food and the
inventory, which the client shows, are written whenever they change). **Inferred**: a
hurt player standing still then commits at each heal (every 10 or 80 ticks), not every
tick; measured in V4.

**The book**: the recipe book (the 1,739 recipes as a bit set, its settings and
highlights), the advancements' progress (each criterion granted, with the date of 4.9),
and the statistics (a sparse table). It is part of the player's state in the region
(`PlayerState`) and crosses with them in `PlayerTransfer`, so a region always has it whole
to award recipes and criteria; a restored region has it from its own state and deltas, as
it has every player's state. It is **not** in every `Has` note (which carries the whole
place): a commit carries a `BookChange` only when the book changed, with **absolute
values** (recipes added, criteria now granted with their dates, each changed statistic's
new value), so that a commit replayed twice changes nothing more. The world store applies a
`BookChange` under the same rule of stay and hops as the `Has` note it travels with
(ADR-0020): a late change from a holder the player has left never overwrites a newer
one. The store keeps the book
beside the place, under the same stay, for a player who is away, and hands it with the
place when they enter (`Entered`). The architecture's `playerdata` service is where this
would live one day; it is an empty crate, and the book stays the store's as the place is
(ADR-0020) until a record says otherwise. Adding the book to `PlayerTransfer` is **the
lead's** (the hand-over).

### 4.3 Damage to a player

One function, the port of `Player.hurtServer` and `actuallyHurt` with
`ServerPlayer.hurtServer`'s PvP and loading checks, in the player's region, used by every
source. It shares the mobs' parts (`mobs/body/hurt.rs`: `CombatRules`, resistance,
absorption) and adds what only players have: the abilities, difficulty scaling, shields,
the helmet, armour wear, the protection enchantments (all five with V4), exhaustion (not
when nothing is left, @90-95), the totem. The damage types become the data's 51 with
their tags, generated from `data/damage_type` and `data/tags/damage_type`, where
`hurt.rs:27-58` names seventeen by hand.

- **Knockback reaches the client**: a hurt that marks the player (`markHurt`, unless
  `no_impact`) sends their own client `SetEntityMotion` (clientbound 103) with their new
  motion, a `PlayerEvent` the edge encodes, in the order the game's tracker sends it.
  The region keeps the player's motion only for that tick (positions stay the client's,
  matrix T0); the client moves itself. A player's hit on a player follows
  `causeExtraKnockback` instead: the motion sent at once to the hit player, and theirs as
  it was afterwards.
- **Sources in the player's tick**, in the game's order (`Entity.baseTick`,
  `LivingEntity.baseTick`, `aiStep`, the blocks' `entityInside` as the player's box
  touches them): fire, lava, drowning (with Respiration's draw), suffocation, freezing,
  cramming (its one-in-four draw), cactus, berry bushes, magma, campfires, fire blocks,
  the effects' ticks (Poison, Wither, Regeneration, Instant Health and Damage, Hunger,
  Saturation, all of which act on players from V4), starving, the void (built).
- **Falling**: the fall distance accumulates from the client's moves as
  `ServerPlayer.checkFallDamage` does with the move's `on_ground` (positions are taken from
  the client, matrix T0); landing hurts by 2.3's rule, on the block's `fallOn` (hay,
  slime, beds, pointed dripstone as the game's).
- **Hits from entities**: mobs' melee, projectiles, explosions, the dragon, falling blocks,
  the ender pearl's landing reach a player of the region directly, and one another region
  holds by `EntityUse::Hurt`, which the holder now takes instead of refusing. The hurt
  carries the source's type, its causing and direct entities and where it came from (for
  the shield's angle and knockback), as `HurtBy` does for mobs.
- **PvP**: a player's attack on another player goes by the same path, refused when the
  `pvp` rule is off (on, its default; question 4). A player's attack carries their
  sprinting (today always `false`, `region.rs:2788`).
- **Death**: the void's death made general: `die` with the combat tracker's message (the
  death messages for each type and the attacker, `death.attack.*` with `.player` and
  `.item` forms, from the client's language keys as the void's is), the items thrown from
  the player's position with `Inventory.dropAll`'s spread (the player's stream), the
  experience as orbs (`min(level · 7, 100)`), Vanishing destroyed. The drops are entities
  in the player's region's land; where they fall into another region's land they cross
  as arrivals, as thrown items do, and can be picked up there once crossed (named, 3.3).
  The respawn resets food and saturation to 20 and 5.

### 4.4 Hunger and regeneration

`FoodData` in the player's tick after the body's (`doTick @42`), as 2.5 reads it: one
timer for the three branches, reset as the game resets it. The exhaustion sources of 2.5
are counted where they happen: jumps and distances from the client's moves (the sprint
and swim from the move's flags and the player's input), attacks and breaks in the region.
Peaceful's regeneration with its saturation as `tickRegeneration`. Eating feeds by
`FoodData.eat(FoodProperties)`, adding the component's saturation as it is, and `canEat`
is asked (`eating.rs` and `using.rs` exist; V4 removes the "food is not kept" difference
named in `eating.rs:21-23`); the Saturation effect feeds by `eat(int, float)`. The edge's
`SetHealth` carries the region's food and saturation, sent on the game's rule of change
(`doTick @146-264`).

### 4.5 Breaking

- **The inputs**: the edge passes START, STOP, ABORT and CHANGE_DESTROY_DIRECTION as
  inputs (`PlayerInput::Dig` becomes the four actions with the face and the sequence) and
  **no longer acknowledges any of them itself** (`play.rs:459-462` does today): each is
  acknowledged by the region after it has acted, and a STOP or START that breaks a block
  across a line by the holder's `RemoteDone`, so that the client's predicted air is
  followed by the true block, never flipped back for a moment, as
  `handlePlayerAction @299-307` acknowledges after the break.
- **The state**: the region keeps `ServerPlayerGameMode`'s fields in `PlayerState`
  (`destroying`, the position and face, the start, the delayed destroy and its position
  and start, the last stage sent), counted against the region's ticks, as the game counts
  its server's (a region that runs slow breaks slow, as a lagging server does). **It moves
  with the player**: in `PlayerTransfer` as ticks passed, and onto the survivor in a
  merge or the part in a split, as cooldowns are (`state.rs:159-161`), so that a dig in
  progress survives every reshaping. Where regions follow players merges happen whenever
  another player comes near; a block coming back at each would be a seam in ordinary
  play. It is not durable: a crash in mid-dig loses it, and the client's STOP then finds
  no dig at that position and only logs, as the game's does (@538-544), so the block stays
  and the client is told it; the player digs again (named: a crash is not ordinary play).
- **Progress** is recomputed each tick, as `incrementDestroyProgress` does, from the
  block's hardness and whether it needs a tool (`Extract.java`, a table in
  `clustine-data`), the tool's rules (`tool` component), Efficiency (V2), the attributes
  and effects (built). `BlockDestruction` goes to the level's other players within 32
  blocks as a region event, by the edge's view as other block events are. START calls the
  block's `attack` (note blocks, redstone ore, the dragon egg).
- **Drops**: `break_by_player` takes the tool and, unless the player is creative, rolls
  the block's loot with it (`match_tool`, Silk Touch, Fortune's `apply_bonus` and
  `table_bonus` with the enchantment's level, shears) and **`copy_components`**, spawns
  the experience of ores (`spawnAfterBreak`), and wears the tool; the statistic and the
  0.005 exhaustion only with the correct tool. Stacks keep the components a broken block
  copies (`container`, `custom_name`, `lock`, `container_loot`, `bees`,
  `pot_decorations`; the matrix's dropped kinds), so a shulker box keeps its contents in
  the item and gives them back when placed; a component this build cannot carry makes the
  break **refused** (the block stays) rather than lose what was in it. What a player breaks
  besides blocks (armour stands, carts, boats, frames, paintings) drops its item in
  survival, as each entity's `hurtServer` does.
- **Across a line**: the player's region checks reach and computes the progress from its
  copy of the chunk (it has the 3×3 around its viewers, ADR-0025), as it checks reach
  now. A START's `attack` on a block across a line is a `RemoteStep::Attack` of its own.
  At the break it sends `RemoteStep::BreakWith { position, expected state, tool }`; the
  holder breaks if the block is still that state, drops with the tool, and its
  `RemoteDone` carries whether it broke and the tool's wear (4.6); the player's region
  applies the wear when the edge passes that on, to the slot **only if it still holds the
  same item** (the same id and components but damage), and to nothing otherwise. A block
  that changed in the meantime is not broken, as the game's STOP would find it changed.

### 4.6 Using up what is used

Every place of 3.1's "items not used up" asks the acting player's
`has_infinite_materials()`. A placed block or a used item shrinks the held stack in the
tick of the use. **Across a line** (every `RemoteStep` that may use an item up: placing,
`UseOn`, `PlaceAt`, `OffHandOn`, a bucket, bone meal, a flint and steel):

1. The player's region **takes what the step may use when it sends the step**: the one
   block or the item (a bucket, a bottle, a stack of bone meal's one), out of the slot,
   into the step (`RemoteStep` carries the taken stack). The player cannot place, throw or
   chest it again in the meantime.
2. The holder (or the region it is passed on to) does the step and puts **the result**
   in its `RemoteDone`: what is to go back (the block not placed, the bone meal not used),
   or what replaces it (an empty bucket for a water bucket poured, a water bottle for a
   bottle filled, a damaged flint and steel), or nothing.
3. `RemoteDone` goes to the edge, as today; the edge passes its result to the player's
   **current** region as an input (`PlayerInput::RemoteResult`), numbered as inputs are,
   so that it follows the player through hand-overs and resumes and is never lost
   (`api.rs:1189-1193`).
4. The player's region puts it back into the slot it came from if that slot is empty or
   holds the same item and has room, else into the inventory, else throws it at the
   player's feet, as `Inventory.placeItemBackInInventory` does.

**Nothing taken is lost on the way** (the lead's decision). Until its result is applied
the taken stack is a **pending entry in `Place`**, keyed by the stay and the step's
sequence: durable with the place, crossing with the player in `PlayerTransfer` and
through merges and splits, and resolved (removed) by the result. A result that arrives for
a player with no stay (they left) goes to the world store's place for them, which applies
it there. A pending entry still in the place when a new stay begins (its result lost with
an old stay) is given back at that stay's start. The result input is exempt from the rule
that a dead player's inputs are dropped (`void.rs:221-237`): it is applied to the dead
player's inventory, which then drops as their death's drops do, or, emptied already by the
death, keeps it for the respawn as their place does.

The client predicts the shrink and sees the correction as the game's `ContainerSetSlot`.
A creative player's step takes nothing and gets nothing back.

### 4.7 Menus

- **The player's own inventory** (window 0) becomes the region's: the edge passes clicks
  on window 0 as on any menu; the 2×2 grid and its result, and the cursor's stack while
  no other menu is open, are in `Place` (durable, question 12), and with them in every
  `PlayerTransfer` and through merges and splits. Window 0 is never closed by a hand-over,
  as the game never closes the inventory menu; the client's screen stays open across one.
  `ContainerClose(0)` returns the grid to the inventory, as `removed` does.
- **Work blocks without a block entity** (crafting table, anvil, enchanting table,
  grindstone, stonecutter, loom, smithing and cartography tables) are menus of the player
  with the block's position, as the game's `ContainerLevelAccess`; `stillValid` asks
  whether the block is still there (the region's own land, or its copy across a line)
  within 8 blocks. Their inputs are in the menu.
- **At a hand-over** another menu is closed as today (`state.rs:140-142`), but first, in
  the old region and the tick of the hand-over, its inputs and the cursor go back to the
  inventory, or are thrown at the player's feet if there is no room
  (`dropOrPlaceInInventory`; named: the throw lands on the old region's side of the line,
  and the item crosses as an arrival), so nothing is lost; the client sees its screen
  shut.
- **A merge or a split moves the player whole**, menus included, as a split does today
  (`region/reshape.rs`'s `split`, 378-386). One case closes: a container menu whose block
  falls outside the player's new region at a split is closed, its inputs and cursor given
  back as at a hand-over, since its block is then another region's. The inventory
  screen's grid and cursor always stay.
- **On a crash**, the grid and the cursor of window 0 are in the place (above); a work
  block's inputs and cursor are in `PlayerState.menu` and in the region's state, and come
  back with it to its last commit, as every menu does now.
- **Block entities with contents** (furnace, smoker, blast furnace, brewing stand,
  campfire) are typed block entities in the chunk (`block_entities.rs:40-63`), durable
  with the chunk's block entities as hoppers' are, ticked in the block entities' phase
  (ADR-0024 6a), with the game's fields (2.10, 2.11). Hoppers and droppers stop treating
  them as shut (matrix T4). Opening one across a pinned line stays as for chests ("opens
  only from its own side", named in T1).
- **Data values** (`ContainerSetData`): the furnace's four, the brewing stand's four, the
  enchanting table's ten, the anvil's cost, the stonecutter's and the loom's selection.
  The edge's synchroniser gains them as it has slots.

### 4.8 Recipes

- `tools/datagen` reads `data/recipe` into Rust tables in `clustine-data`, as loot tables
  are: each recipe's key, type, group and category, its pattern or ingredients (items
  and tags), its result with components, and the cooking time and experience. The
  special recipes are code, ported from their classes.
- Matching is `RecipeManager.getRecipeFor`'s: the first recipe of the type that matches,
  in the manager's order (`RecipeMap`'s, to be read: a recipe that two match must give
  the game's). No hash maps: a sorted table by the ingredients' items.
- The edge sends `UpdateRecipes` (property sets and stonecutter recipes) at the join,
  from the generated tables.

### 4.9 The recipe book and advancements

- An **advancement engine** in the simulation: the advancements' criteria from
  `data/advancement`, a trigger fired where the game fires it (the player's inventory
  change, a kill, a location every 20 ticks), progress in the player's book, rewards
  (recipes, experience, loot) given in the player's region. A trigger that happens in
  another region (a kill by an arrow across a line) reaches the player's region with the
  effect that caused it, as the kill's experience does (**Inferred**: to be listed per
  trigger).
- **The dates** a criterion is obtained are the wall clock in the game. The simulation has
  no clock; the worker passes **one wall time per tick** in with its inputs, and every
  date of that tick is it, so a replay gives the same date. Comparisons with the official
  server cannot be exact on dates (named).
- V8 builds the engine with the recipe advancements (`inventory_changed`,
  `recipe_unlocked`, `enter_block`, `tick`, `impossible`), the book's packets, the
  `PlaceRecipe` fill and the settings. V12 adds the 126 shown ones and their triggers,
  `UpdateAdvancements` with their display, toasts, and `SeenAdvancements`.

### 4.10 Statistics

Counters in the book, set in the region where they happen, sent on the client's request
(`AwardStats`). What changes every tick (`play_time`, `total_world_time`,
`time_since_death`, `time_since_rest`, `crouch_time`) is derived from ticks passed, and the
distances (`walk_one_cm` and the rest) are added once a second from the region's journal,
written as absolute values (**Inferred**; question 6).

### 4.11 Mobs and players

Once the face carries the real mode, the targeting of 3.1 is right as it stands. What
changes: a survival player hit is hurt and knocked back (4.3); `ender_dragon.rs:428`'s
filter lets the wings and head reach them; raiders, illagers and golems attack; monsters
near a bed keep a survival player awake (`sleep.rs:24-25`). **Where regions follow
players**, a monster targeting a player of another region makes the two merge: V4 extends
`target_at_line` (`region/arrivals.rs:127-143`) to a target that is a player's face, noting
the chunks about the mob and the player for the holder, as it does for a mob's target.
Across a **pinned** line, the hit is an `EntityUse::Hurt` a tick late (named).

### 4.12 Determinism and randomness

Every draw the game takes from the player's random (`getRandom()`: `dropAll`'s spread at
death, the anvil's chipping, Respiration's skipped breath, cramming's one in four, the
enchanting table from `XpSeed`) comes from **one stream per player per tick**, made once in
the player's tick (`Random::of_entity_aside`, keyed by world, entity, lived and tick,
`random_ticks.rs:100-110`) and passed through every use in the game's order. Building it
twice in one tick would draw the same numbers twice; a use outside the player's tick (a
remote result, a menu click) builds it with a mark of its own mixed in. The furnace's
experience draws from the level's random, which is the region's stream. The enchanting
seed is durable in the place; a new player's is drawn at their making. The official
server's draws are clock-seeded, so comparisons are exact only where nothing is drawn
(section 6).

### 4.13 What crosses a line, in one place

| What | Same region | Across a line |
|---|---|---|
| A mob's or a projectile's hit, an explosion on a player | direct, knockback sent | `EntityUse::Hurt` to the player's holder, a tick late while not coupled |
| A player's hit on a player | direct | `EntityUse::Hurt` (as a hit on a mob is now) |
| A monster chasing a player | | merge where regions follow players (V4); a tick late across a pinned one |
| Breaking | direct | progress from the copy; `RemoteStep::Attack` and `BreakWith`; acknowledged and worn on `RemoteDone` |
| Placing, using an item on a block | direct, stack shrunk | item taken when sent, result back by `RemoteDone` through the edge |
| Death's drops and orbs | spawned | arrivals, picked up once crossed |
| A work block's menu | open | from the copy for the blocks without contents; a container's from its own side only (named) |
| The recipe book, advancements, statistics | the player's region | triggers from elsewhere come with the effect that caused them |
| A hand-over | `PlayerTransfer` with the place, the book, the dig in progress, window 0's grid and cursor; another menu emptied into the inventory and closed | |

### 4.14 The edge

New packets and fields: `GameEvent` CHANGE_GAME_MODE, abilities and the list entry from
the player's mode; `SetHealth` with food; `SetEntityMotion` to a knocked-back player;
`BlockDestruction`; acknowledgements of breaking from the region; `ContainerSetData`;
window 0's clicks and `ContainerClose(0)`; `ContainerButtonClick`, `RenameItem`,
`SetBeacon`; `UpdateRecipes`, `RecipeBookAdd`/`Remove`/`Settings`, `PlaceRecipe`,
`PlaceGhostRecipe`, `RecipeBookChangeSettings`, `RecipeBookSeenRecipe`;
`UpdateAdvancements`, `SeenAdvancements`, `SelectAdvancementsTab`; `AwardStats` on
`ClientCommand(1)`; `PlayerLoaded` as an input; a `RemoteDone`'s result passed on as an
input; the death messages' forms. The edge's hand-over and resume (`hand_over`, `placed`,
`enter_by_arrival`, resuming) change only where `PlayerTransfer` gains fields (the book,
the dig, the grid): the lead's, in V1.

## 5. The steps, in order, and what each leaves a player

Each is one verified commit or a few, tests from the specification by someone who did not
write it, comparisons where marked. The default game mode stays creative until V5; the
end-to-end tests that exist run creative throughout (they pass `--gamemode creative`
once the default changes).

**The survival core**

- **V0** This plan, its review, the owner's answers; the classes of section 9 read.
- **V1 The game mode and the formats** (the lead's: contracts, formats, the hand-over's
  fields). `GameMode` and the abilities; **every new field of V defined at once** (section
  7): the place's, the book and `BookChange`, the grid and the cursor, the dig in
  `PlayerTransfer`, the stacks' copied components, the furnaces', brewing stands' and
  campfires' block entities, the world's default mode; `--gamemode`, `--force-gamemode`;
  the face's and the edge's mode; refusals at the edge and in the region; reach from the
  attributes; `PlayerLoaded`; the empty start; the operator's tool if question 2 says so;
  every creative assumption of 3.1 reading the player's abilities, **except** damage and
  targeting, which stay off until V4 (a survival player is invulnerable in V1 to V3, named
  while it lasts). *Leaves*: a player started in survival who cannot fly or take from the
  creative tab, whose items are used up and picked up as the game does, and who **cannot
  yet break anything properly**; so V1 and V2 land together before anyone plays.
- **V2 Breaking**: hardness and the correct tool from the jar; the `tool` component;
  progress, the four actions, the delayed destroy, `BlockDestruction`, the region's
  acknowledgements; the dig carried through hand-overs, merges and splits; drops with the
  tool (`match_tool`, Silk Touch, Fortune, shears), `copy_components` and the shulker
  box's contents; Efficiency; the experience of ores; tool wear and its breaking; Haste,
  Mining Fatigue, water and air; `attack` on a punch; entities broken with their drops;
  across a line, with items taken and given back (4.5, 4.6); the edge puts the copied
  component kinds on the wire and hashes them for `ContainerClick`'s hashed stacks, as it
  does the 17 kinds it has (`components_oracle.rs`). *Leaves*: a player who
  gathers wood, stone and ores with the game's times and drops, with the tools villagers
  sell working. Comparisons: the times and packets of breaking a dozen blocks with a dozen
  tools (exact: nothing is drawn but Fortune's).
- **V3 Crafting**: window 0 driven by the region, the 2×2 grid, the crafting table,
  the recipes from the jar with the special ones, `ResultSlot`'s remainders and quick
  craft, `UpdateRecipes`; the grid and cursor through hand-overs. *Leaves*: logs to
  planks to a table to tools; a peaceful survival. Comparisons: a craft of each recipe
  type, packet by packet.
- **V4 Health**, in three commits landed together before anyone plays, so that no build a
  player meets has damage without healing: **V4a** damage and knockback (4.3 whole: every
  source of 2.3 but lightning and the world border; armour, toughness, the five
  protections, shields, the helmet, the totem; difficulty scaling; the effects on players;
  mobs, the dragon and golems attack; the merge for a chase at a line; PvP; invulnerable
  while loading); **V4b** hunger and regeneration (4.4: `FoodData`, exhaustion from every
  source, the three branches, peaceful's, eating fed and refused, the `SetHealth` rule);
  **V4c** death with drops and experience, the messages by type, the respawn's reset.
  Measured: what a hurt player at rest and a sprinting one cost the log. *Leaves*: the
  game's danger, hunger and healing. Comparisons: falls from heights, a zombie's and a
  skeleton's hits on easy in each armour with the motion sent, lava, drowning, a creeper
  (exact but the creeper's rays and the arrow's crit), a sprint, jumps and a meal tick by
  tick, death and its drops.
- **V5 Furnaces**: furnace, smoker and blast furnace as block entities with their menus,
  fuel from `cooking_fuel` and its providers with the fast blocks' halving, the speed,
  the rescaling on fresh fuel, experience on taking and on breaking, the `lit` state,
  hoppers in and out; campfires cooking. **The default game mode becomes survival**
  (question 1), and the default world a seeded one (question 14). *Leaves*: iron, cooked
  food, the first hour of the game. Comparisons: a furnace's run with coal, with a lava
  bucket and in a smoker, its data values tick by tick.

The owner tries the core with real clients here; what to try goes in the roadmap's first
section, and V6 begins.

**The rest**

- **V6 Enchantments' effects**: every effect kind of 2.14 that has something to act on
  and was not built with V2 and V4: damage, post-attack, attributes, Thorns, Fire Aspect,
  Knockback, Looting, Frost Walker, Soul Speed, the curses, Mending with orbs, Breach,
  Density, Wind Burst. *Leaves*: enchanted gear from trades and loot that works.
- **V7 The enchanting table, the anvil, the grindstone**: their menus, the seed, the
  bookshelves, costs, renaming. *Leaves*: experience has a use.
- **V8 The recipe book**: the advancement engine with the recipe advancements, the book's
  packets, `PlaceRecipe`. *Leaves*: the book fills as they gather.
- **V9 Brewing**: the brewing stand, its recipes and fuel, hoppers. *Leaves*: potions
  made.
- **V10 The stonecutter, the loom, the smithing table**; the cartography table's menu
  that takes items and makes nothing until maps (question 10).
- **V11 The beacon** (menu, levels, effects in range; question 13).
- **V12 Advancements shown**: the 126 with their triggers, toasts, the screen, the
  rewards. Those whose triggers need what is not built (fishing, lightning, spawners,
  the wither) cannot be had, named.
- **V13 Statistics**: 4.10.
- **X** Tests from the specification of every step by someone who did not write it; the
  comparisons, written by a subagent and run by the lead with `tools/oracles.sh`.

**Division of work**, as P did it: V1, the dig's and the grid's crossing (V2, V3) and the
results through the edge (V2) are the lead's; the rest is delegable by family once V1's
contracts are pushed, each with files of its own (breaking, damage sources, hunger,
recipes and crafting, furnaces, brewing, each work block, advancements). V2 and V4 both
touch `region.rs`'s input handling and `updates.rs`; they are not run side by side.

**Size** (**Inferred**, by P's measure of 62,000 to 90,000 lines): the core 35,000 to
50,000 lines with tests, the rest 30,000 to 45,000.

## 6. Tests and comparisons

- **From the specification**: each step's tests written from this plan and the game's
  classes by someone who did not write the code, as at E6, R7, L5: the damage formula in
  a table of cases (armour, toughness, Protection levels, Resistance, difficulty, the
  zero that skips exhaustion); fall heights at the boundaries (3, 3.000001, 4); the
  breaking time of each tool tier on stone, obsidian and wool, with Efficiency, Haste,
  Mining Fatigue, in water and in the air, and with a tool changed in mid-dig; the food
  timer shared by its branches; recipes that two patterns could match; the furnace's
  rescaling and its timer falling when unlit; the anvil's costs.
- **Ordering, by someone who did not write V2 to V4**: a dig in progress through a
  hand-over, a merge and a split (the block breaks once, at the game's time); a placing
  across a line refused and accepted, with the last block of a stack placed again at
  once elsewhere (never two); a bucket used across a line; a death during a hand-over and
  during a merge (drops once); a hit that arrives at the old holder after the player left;
  a shulker box broken and placed again; a crash in mid-dig, with items in the grid, with
  a stack on the cursor.
- **End-to-end** tests in `bin/clustine/tests` for what a player sees, in both layouts.
- **Comparisons with the official server**: `server.properties` per test
  (`gamemode=survival`, or creative bots told `/gamemode survival <bot>` on the console);
  items given by `/give` on the official side and by a test kit on Clustine's (the
  starting inventory made settable, or the operator's tool of question 2); bots learn
  timed digging (START, wait for the progress, STOP), the inventory screen, crafting
  clicks, the respawn, following food, saturation and their own motion. Exact where
  nothing is drawn: breaking times and packets, fall damage, armour's arithmetic, a
  zombie's hit on easy and its knockback, hunger by tick, crafting, a furnace's run, the
  anvil's cost. By bounds or distribution: death's drops' spread, enchanting offers
  (seeded by a seed the official server draws from the clock), the anvil's chipping,
  Fortune, explosions, cramming.
- **The old tests**: the end-to-end tests stay creative (`--gamemode creative`), so that
  V changes none of them by its default; `join.rs:49` asserts the default it is given.

## 7. Formats that change

**A raise at each step that needs one** (the owner's answer to question 8, 2026-11-03;
the plan had proposed one raise in V1): each step defines the fields below that it uses,
raises the version with its known-answer tests, and refuses older worlds. Where the
fields go, in the order the steps come to them:

- `FORMAT_VERSION` (23): the place's new fields (4.2), its pending entries (4.6) and window 0's grid and cursor; a new
  `LoggedBook` and the commit's `BookChange` (absolute values); stacks' copied components
  (`container`, `custom_name`, `lock`, `container_loot`, `bees`, `pot_decorations`) need
  **no raise**: since format 14 kinds are stored by name and a kind a build does not know
  is skipped (`clustine-format/src/lib.rs:55-56`). Named: an older build reading them,
  as during M4's rolling updates, drops a `container` it does not know, and the contents
  with it; the world's default game mode in `meta`; the attack and use effects' flags
  carrying the player's abilities in place of bit 0's "creative"
  (`clustine-format/src/effects.rs:13`); the block entities of furnaces, smokers, blast
  furnaces (the game's six fields and `RecipesUsed`), brewing stands (its four and the
  speed) and campfires (the two arrays). Each with its known-answer tests, as
  `lib.rs:40-71` records each raise.
- `STATE_FORMAT` (31): `PlayerState` (the dig, the counters, the book) and
  `PlayerTransfer`, the remote steps' taken stacks and `RemoteDone`'s result, and the
  block entities in the region's state.
- The steps that raise: V1 (the game mode in the place and `meta`, the effects' flags),
  V2 (the dig in the state; the pending entries of 4.6), V3 (window 0's grid and
  cursor), V4 (the place's counters), V5 (the furnaces' block entities), V8 and V12 (the
  book), V9 (brewing stands); a step may find it needs none, and says so. The
  worker's `STATE_FORMAT` is 31 since the poses of 2026-11-02.
- The generated data grows: block hardness and tool requirement, the item components
  `tool`, `cooking_fuel`, `brewing_fuel`, `enchantable`, `equippable`'s damage on hurt,
  `blocks_attacks`, `repairable`, `provides_banner_patterns`, recipes, advancements,
  damage types and their tags, the cooking providers.

## 8. Risks

- **The size**: survival is the largest phase after P, and touches a hundred and twenty
  places of the simulation (3.1). Ordering by family and files of their own matters more
  than in P.
- **The log**: the place in every `Has` grows by a few dozen bytes; a hurt or hungry
  player commits more often (4.2). Measured in V4 against N's and P's numbers.
- **Breaking across a line and lag**: progress counted in a slow region's ticks makes
  breaking slow there, as the game's on a lagging server; a block across a line that
  changes between the copy and the holder is not broken, and the client shows it come
  back.
- **Client prediction**: the client predicts breaking, placing and the shrink of stacks
  (not crafting results, which it waits for in 26.3); every disagreement is a correction
  the player sees. The comparisons are the judge.
- **A death in a hand-over or a reshaping**: a hurt arriving at the old holder after the
  player left is passed over (the game would have hurt them); a death during a merge or a
  split must drop once (section 6).
- **Mobs across a line**: a chase across a pinned line leaves the mob hesitating at the
  line (ADR-0028's named risk), now with a player who can be hurt.
- **The tests**: every existing end-to-end test assumes creative; the default's change in
  V5 must not reach them (section 6).

## 9. Classes to read before the steps

Readable now from the jar with the review's disassembler (no Java), before V1:
`CraftingMenu`, `AbstractCraftingMenu`, `RecipeManager`, `RecipeMap`, `ShapedRecipe`,
`ShapelessRecipe`, `TransmuteRecipe`, the special recipes, `ServerRecipeBook`,
`ServerPlaceRecipe`, `AbstractFurnaceMenu`, `FurnaceResultSlot`, `BrewingStandMenu`,
`BrewingRecipe`, `EnchantmentMenu`, `AnvilMenu`, `ItemCombinerMenu`, `GrindstoneMenu`,
`StonecutterMenu`, `LoomMenu`, `SmithingMenu`, `CartographyTableMenu`, `BeaconMenu`,
`PlayerAdvancements`, `ServerStatsCounter`, the criteria triggers used,
`DropExperienceBlock`, `CactusBlock`, `MagmaBlock`, `PowderSnowBlock`, `AnvilBlock`,
`Tool`, `ServerEntity.sendChanges` (the motion to a hurt player), `Player`'s interaction
ranges, `ServerPlayer.startSleepInBed`'s monster box. Read already in this revision:
`FoodData`, `DedicatedServer`, `DedicatedServerProperties`,
`AbstractFurnaceBlockEntity`'s fields, `BrewingStandBlockEntity`'s, `CampfireBlockEntity`'s,
`ServerPlayer.tickRegeneration`.

## 10. Cost and measurement

- The log and commits a second with players hurt, hungry, sprinting and digging (V4),
  against a creative player's.
- A furnace array of 64 running (V5), as R8 measured circuits.
- A crowd of monsters chasing survival players across a line where regions follow
  players: merges and their pauses (V4).

## 11. Questions for the owner

**Answered on 2026-11-03**: the owner agreed every recommendation below but question 8's,
where they chose (b), a raise at each step that needs one, refusing their world each time.
The questions stand as they were put.

1. **How a player becomes survival.** (a) The server's default game mode, `--gamemode`, as
   the game's `server.properties` `gamemode`, kept in the world and given to new
   players, each player's mode then their own and saved, with `--force-gamemode` to
   apply the default to returning players; (b) a world setting fixed at the world's
   making only; (c) every player always in the flag's mode, nothing saved.
   **Recommended: (a)**, as the game. And when does the default become `survival`? (i)
   at V5, once a player can play the first hour; (ii) at V1, so that you play survival
   as it grows; (iii) never: `--gamemode survival` asked for. **Recommended: (i)**, with
   `--gamemode survival` usable from V1 to watch it grow.
2. **Changing a player's mode while playing**, with no commands. (a) None: the flag and
   `--force-gamemode` at the next join; (b) an operator's tool: a `clustine` subcommand
   (`clustine gamemode <player> <mode>`, `clustine give <player> <item> [count]`) that
   talks to the cluster as `clustine move` does, through the coordinator to the player's
   region, so it works the same on one machine and on Kubernetes (`kubectl exec`), and
   in the single binary also on its standard input; it serves the comparisons' kits too;
   (c) the commands milestone first. **Recommended: (b)**.
3. **Difficulty.** It is fixed at easy (`difficulty.rs:19`), whose record says another
   needs the enchantment providers of mobs' equipment first; peaceful also removes
   monsters, and normal and hard change many mobs' rules beyond damage. (a) Easy only in V,
   with `--difficulty` a later step of its own that audits the 31 places that read it;
   (b) `--difficulty` in V4 with that work. **Recommended: (a)**.
4. **Game rules**, including `pvp` (a game rule in 26.3, not a property),
   `keepInventory`, `naturalRegeneration`, `fallDamage`, `fireDamage`, `drowningDamage`,
   `freezeDamage`, `doImmediateRespawn`, `showDeathMessages`, `limitedCrafting`,
   `maxEntityCramming`: at the game's defaults (PvP on) and not changeable until commands
   (proposed), or flags for some now (`--no-pvp`, `--keep-inventory`)? **Recommended**: the
   defaults, named.
5. **Advancements.** The recipe book needs the recipe advancements (V8). The 126 shown
   ones are what a player sees as toasts and a screen; the game gives them in creative
   too. (a) Both, the shown ones late (V12); (b) only the recipe ones, the shown ones
   with a later tier; (c) none: every recipe unlocked at the join. **Recommended: (a)**.
6. **Statistics.** (a) Kept and sent on request (V13), distances added once a second, so
   a crash may lose a second of distance; (b) every tick, as everything else is; (c) not
   built: the request answered with nothing. **Recommended: (a)**.
7. **What crosses a line.** A hit across a pinned line a tick late; breaking across a
   line judged on the copy, acknowledged and worn when the holder answers; items taken
   when a step across a line is sent and the rest given back; where regions follow
   players, a monster chasing a survival player across a line merges the two. An item
   taken for a step across a line is a pending entry in the player's place until its
   result comes back: durable, crossing with them, given back at a new stay if the result
   was lost, and applied even to a dead player, whose death then drops it (4.6). Agreed?
8. **Formats.** (a) **One raise in V1** with every field V needs (section 7), so that a
   world made after V1 survives the rest of V, yours included; (b) a raise at each step
   that needs one, refusing your world each time (three or four times). Either way the
   world made before V1 is refused once. **Recommended: (a)**.
9. **One phase or two milestones**: the core (V1 to V5) and the rest (V6 to V13), with your
   trial between? **Recommended: two**.
10. **The cartography table** without maps: its menu takes items and makes nothing until
    maps are built (proposed), or left inert with maps? And maps themselves (a
    `filled_map`, its data, its packets), a gap the matrix does not list: with V, or
    later? **Recommended**: the menu with V10, maps later as a row of the matrix.
11. **Adventure and spectator.** Proposed out of V: `--gamemode` refuses them until built
    (spectators unseen and through blocks with the camera; adventure's
    `can_break`/`can_place_on`). Or in V, after V5? **Recommended**: out, as a row of the
    matrix.
12. **Open screens across a hand-over and a crash.** Proposed: window 0's grid and cursor
    durable in the place and carried across hand-overs (its screen stays open); another
    menu (a crafting table, a furnace) emptied into the inventory and closed at a hand-over
    to another region (the client sees its screen shut; what does not fit is thrown on the
    old region's side); a merge or split moves the player whole with their menus, but a
    container menu whose block falls outside their new region at a split is closed and
    its cursor given back; the inventory screen's grid and cursor always stay. Or carry every open menu across a hand-over too (a menu of a block across the line
    then works from a copy, and a container's from its own side only)? **Recommended**:
    as proposed; carrying every menu later if a hand-over at a crafting table is seen in
    play.
13. **The beacon** with V (V11), or with conduits later as the matrix has it?
    **Recommended**: with V.
14. **The default world.** A flat world has no trees or ores (`bin/clustine/src/lib.rs:102-106`),
    so a survival player there can do nothing. When survival becomes the default (V5),
    (a) `cargo run` without `--seed` makes a seeded world with a seed drawn at its making,
    as the game's `level-seed` left empty does, and the flat world is asked for by a flag;
    (b) the flat world stays the default and survival needs `--seed`. **Recommended: (a)**.
15. **A new survival player's start.** Empty, as the game's (proposed), the hotbar of nine
    blocks given only in creative? **Recommended**: empty.

Spawn protection is not a question: the game applies it only while the server has
operators (`DedicatedServer.isUnderSpawnProtection @21-34`), and Clustine has none.

## 12. Review (2026-11-02), and what changed

An independent review of the first draft found the game facts it checked right (the hurt
order, `CombatRules`, magic, scaling, START/STOP and 0.7, the destroy speed, movement
exhaustion, packet ids, recipe counts, the inventory menu, Unbreaking, the brewing stand's
data) and asked for another round on the design. Each finding and what changed:

**High**

1. *Placing across a line duplicates items*; `RemoteDone` carries no result. 4.6 is
   rewritten: the item is taken when the step is sent and carried in it; `RemoteDone`
   carries the result (given back, replaced, nothing); the edge passes it to the player's
   current region as a numbered input; it lands in the slot, the inventory or at the feet.
   The same for every step that uses an item up. 4.13, 0.4, section 6 and 7 follow.
2. *A shulker box broken in survival loses its contents*. 2.6 and 3.2 say so;
   4.5 rolls `copy_components` and stacks keep `container`, `custom_name`, `lock`,
   `container_loot`, `bees`, `pot_decorations`, in V2, with a break refused rather than a
   component lost. Section 7 defines them in V1.
3. *A hit player is never knocked back*. 2.2 adds `markHurt` and `SetEntityMotion` to the
   player; 4.3 sends it; 4.14 lists it; V4a and the comparisons cover it.
4. *The breaking state*. 4.5 is rewritten: the dig moves with the player in
   `PlayerTransfer` and through merges and splits; a STOP at another position only logs
   (2.6, 4.5's crash case corrected); the region acknowledges breaking, never the edge
   first; CHANGE_DESTROY_DIRECTION is an input. Section 6 tests a dig through each.

**Medium-high**

5. *A chase across a line does not merge today*. 3.1 cites `target_at_line`; 4.11 makes
   the extension V4a's work.
6. *Menus at a hand-over*. 4.7 is rewritten: window 0's grid and cursor durable and carried
   (its screen stays open); another menu emptied into the inventory (or thrown) in the old
   region before it closes; a crash's case for each. Question 12 is rewritten.
7. *Missed creative assumptions*. 3.1 adds reach, sleep with monsters near, pick-block,
   the pearl's 5, instant effects, Regeneration and Hunger on players, raids' living
   heroes, the starting hotbar, invulnerability while loading, `attack` on a punch (note
   blocks, across a line a step of its own), hives, cushions, the attack's sprinting.
   4.1, 4.3, 4.5 and 4.11 build each.

**Game facts**

- PvP is the game rule `pvp`, not a property: 2.1, 2.2, 4.3; question 11 folded into 4.
- Spawn protection applies only with operators: 2.1; question 14 removed (a statement at
  the end of section 11).
- Food: `eat(FoodProperties)` adds the component's saturation as it is; fast regeneration
  adds `min(saturation, 6)` exhaustion; one timer for the three branches; peaceful raises
  saturation: 2.5 rewritten from `jar:FoodData` and `jar:ServerPlayer.tickRegeneration`;
  4.4 follows.
- `bypasses_cooldown` added to 2.2; the return before exhaustion when nothing is left
  added to 2.2 and 4.3.
- Cramming's draw: 2.3, 4.3, 4.12.
- Flying is not kicked: 2.1, 4.1.
- Furnaces: the fast blocks halve the burn time; each fuel's speed; the rescaling; the
  hash map's order and the level's random for experience: 2.10, 4.12, V5.
- Breaking: statistic and exhaustion only with the correct tool; progress recomputed each
  tick: 2.6, 4.5.
- Peaceful's saturation: 2.5.
- The client cannot predict crafting in 26.3: 2.9 and section 8 corrected.
- The guesses read: soul fire 2, air 300, the properties' defaults, the forced mode and
  its writing at every start, the explosion's `+1`, `handleSetCreativeModeSlot`, the save
  data's fields: 2.1, 2.3, 4.1, 4.2; section 9 lists only what is left to read.

**Design in Clustine's terms**

- Counters as what is left, not ticks begun, and what a crash gives back named: 4.2.
- `BookChange` in absolute values; the book in the region's state and transfers, so a
  restored region has it; the store's copy for absent players: 4.2. The advancement
  dates from the tick's inputs: 4.9.
- One stream per player per tick, Respiration's draw: 4.12.
- The region refuses creative-only inputs too: 0.2, 4.1.
- Drops across a pinned line picked up once crossed: 3.3, 4.3.
- Wear arriving on `RemoteDone` lands only on the same item: 4.5.

**Steps**

- Regeneration: V4 now has damage, hunger and death as three commits landed together
  (V4a to V4c), so no build a player meets hurts without healing; V5 is the furnaces.
- Efficiency in V2, with Silk Touch and Fortune, since villagers sell such tools: 2.14, V2.
- Difficulty: question 3 rewritten from `difficulty.rs:1-6` (the enchantment providers,
  peaceful's monsters); easy only in V recommended.
- Spectator and adventure scoped out: 0.1, 1.1, 4.1, question 11.

**Questions**

- Question 8 recommends one raise in V1, with section 7 listing every field.
- Question 4 takes PvP as a game rule; spawn protection is no question; question 12 covers
  the cursor, work-block menus and a hand-over's effect on an open screen; question 2 says
  how the operator's tool reaches a cluster (a subcommand through the coordinator, as
  `clustine move`).
- Added: question 11 (adventure and spectator), 14 (the default world seeded), 15 (an
  empty start).

**Minor**: the pearl's 5 in 2.3; the attack's missing sprint in 3.1; no experience with
`keepInventory` or for spectators in 2.4; section 9 trimmed.

**The second reading (2026-11-02)** found the plan ready for the owner after these
changes, made in this revision:

1. *An item taken for a step across a line could be lost* if the stay ended or the player
   died before its result (the lead's decision): 4.6 keeps it as a pending entry in
   `Place`, keyed by stay and sequence, durable and crossing, resolved by the result; a
   result for a player with no stay goes to the store's place; a pending entry at a new
   stay's start is given back; the result is applied to a dead player too, exempt from
   `void.rs:221-237`. Question 7 says so.
2. *4.7 contradicted itself on merges and splits*: a merge or a split moves the player
   whole; a container menu whose block falls outside the new region at a split is closed
   and its cursor given back (today a split moves the menu with the player,
   `reshape.rs` 378-386); the grid and cursor stay. 4.7 and question 12.
3. *A late `BookChange`*: the store applies it under the stay-and-hops rule of the `Has`
   note it travels with (ADR-0020): 4.2.
4. *Copied component kinds*: no raise (kinds by name since format 14, unknown skipped);
   an older build drops an unknown `container`, named; V2 puts the kinds on the wire and
   hashes them: section 7, V2.
5. *Knockback's citations*: `n7:Entity.markHurt @1`, `n5:ServerEntity.sendChanges
   @960-993`; a player's attack by `n7:Player.causeExtraKnockback @124-166`, which PvP
   follows: 2.2, 4.3.
6. *Reach read*: `n6:Attributes.<clinit>` @214 and @350, the creative modifiers (0.5 and
   2.0, `n7:Player` line 41), from the eyes to the box: 2.1.
7. *The date*: one wall time per tick from the worker; no exact comparison of dates: 4.9.
8. A menu emptied at a hand-over throws on the old region's side (4.7, question 12); the
   loading invulnerability restarts after a respawn (`handleClientCommand @105`): 2.1,
   4.1; an operator's `give` or `gamemode` is a new kind of region input, not a
   `PlayerInput`: 4.1.
