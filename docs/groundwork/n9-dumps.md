# N9 brief against the n9 dumps: what the bytecode settles

Reads the **Inferred** claims of `docs/groundwork/n9-brief.md` against the classes dumped in
`/home/claude/reference/n9-javap/` (107 classes, `javap -p -c -constants`; `SpawnPlacements-v.txt`
is `javap -v`). "n9 X lines a–b" are line numbers of that class's `javap` file; "brief 4.2 line
636" is the brief's section and line. Other dumps are cited by their directory (n5, n6, n7, n7k,
n7d). Data files are from `target/datagen/26.3/generated/`. Verdicts: **CONFIRMED**, **CORRECTED**,
**STILL UNKNOWN**. Where `javap -c` does not show something (a lambda behind an `invokedynamic`
index, an id given by definition order), it says so.

Sections 1 to 3 were read by two forked readers and spot-checked (the arrow's REVERSE scale, the
crossbow's use duration, the pearl's particle draws); sections 4 to 6 directly.

## Summary of the corrections

1. **A refused arrow** reverses at a tenth of its speed (`deflect(REVERSE, …, 0.2)`: motion ×
   −0.1, yaw + 170 + 20·f, one own `nextFloat`), its fire ticks restored, and lives on; it is
   discarded (dropped if ALLOWED) only if motion² < 1e-7 afterwards. Not "× 0.2 and may drop"
   (1.2 rows 1, 4).
2. **The enderman refuses every projectile** (`#is_projectile` → up to 64 teleports, false) with
   `projectileReceivesSideEffectsOnHit(false)` false: no bounce, the arrow flies on, and the
   enderman keeps the 5 s of fire the arrow set **before** the hurt; the arrow has no enderman
   test (1.2 row 11; 5.6).
3. **Arrows hit every entity along a step** (`getManyEntityHitResult(…, false, false)`, sorted by
   distance); only the trident takes the nearest one (1.2 row 7). The arrow count rises only for
   pierce ≤ 0; the hit sound plays for any target the hurt took on.
4. **The trident** deals a flat 8.0f (no speed factor, no crit, no arrow count), sets
   `dealtDamage` before the hurt, and bounces by REVERSE (0.02, 0.2, 0.02) with one own draw
   **whether or not the hurt took**; water inertia 0.99f (1.2 rows 2, 10, 14).
5. **Evoker fangs**: entity event 4 at the first tick of negative warmup, seven ticks before the
   damage; discarded 23 ticks after the warmup ran out; an ownerless fang hurts by `magic` (1.2
   row 6).
6. **A firework without components flies** (flight 1, lifetime `10 + nextInt(6) + nextInt(7)`) and
   pops harmlessly; `FireworkRocketItem.use` works only while fall flying, otherwise a rocket is
   placed on a block (1.2 row 17; 3.8).
7. **The crossbow's use duration is 72000**; the charge is 25 ticks; it **loads in `onUseTick`**
   when progress reaches 1, not in `releaseUsing` (a held use loads on the 26th tick; the
   pillager's release loads it through the extra use tick of `useOnRelease`); the end sound draws
   one level `nextFloat` before the goal's `nextInt(20)` (3.1, 3.4).
8. **The crossbow shot**: `crossbow_shoot` for every projectile; crit exactly for a player; all fly
   at 1.6 if any charged item is a firework; projectiles after the first draw a pitch on the
   shooter's random after their six (3.4, 3.9).
9. **Dispensers** shoot from the centre + 0.7·facing **+ 0.1 in y** (fireworks centre +
   0.50001·facing) (2.3).
10. **The splash** measures box to box (the potion's box moved to the hit point against each
    target's box inflated by `computeMargin`): a direct or touching hit is proximity 1; durations
    below 20 after scaling are dropped; 2007 only for a base potion with instant effects, and a
    separate level event 1054/1053 for the sound; the water behaviour uses the unmoved box with
    centre distance (axolotls: no distance test); a lingering water bottle makes no cloud; the
    harm of a thrown potion is `indirectMagic(potion, owner)` (2.4; 4).
11. **The lingering cloud** shrinks 0.005 a tick, takes the item's 0.25 duration scale, is placed
    at the hit entity when one was hit, and draws nothing on the server (2.4, 2.5).
12. **The ender pearl** makes 96 own draws on every hit (32 portal particles evaluated on the
    server), then for a player owner one own `nextFloat() < 0.05` (endermite only if monsters
    spawn and not peaceful); 5.0 damage; the 20-tick cooldown is the item's `use_cooldown`
    component (2.8).
13. **The experience bottle**: `3 + nextInt(5) + nextInt(5)` is the experience total, split into
    orbs by `awardWithDirection` (not dumped), not the number of orbs (2.7).
14. **The egg's chicks** take the egg's yaw and get no `finalizeSpawn`; a failed size fudge stops
    further chicks (2.6).
15. **`spawnEnemies` does not read the difficulty** (`isSpawningMonsters` only); peaceful is
    refused per type in `SpawnPlacements.checkSpawnRules` (5.4).
16. **Phantoms come to creative players**: `PhantomSpawner` skips only spectators, and
    `TIME_SINCE_REST` counts in every game mode; after 72 000 ticks without sleep, at night, above
    sea level under open sky (5.4).
17. **Patrols**: no day test in the spawner; the `early_game` timeline's
    `can_pillager_patrol_spawn` (false until tick 120000) does it; patrols start in **daylight**
    (5.4).
18. **The slime's spawn rule** (5.5): the surface branch is one level `nextFloat()` against the
    environment attribute `surface_slime_spawn_chance` (0.5 × the moon's brightness, from the
    `moon` timeline), in `#allows_surface_slime_spawns`, then darkened light `<= nextInt(8)`; a
    failure **falls through** to the slime-chunk test, which **always** draws a level
    `nextInt(10)` before looking at the chunk or the height; the slime is an `AgeableMob`
    (`monster.cubemob`), its size at data **18**; `finalizeSpawn` adds `nextInt(3)` and (size
    below 4) one `nextFloat()`.
19. **The sulfur cube's spawn rule is `true`**, and Clustine has no sulfur cube: sulfur caves'
    positions are silenced as the slime's and enderman's are (5.7, 5.8).
20. **Other mobs never block a spawn box**: `noCollision`'s entity part counts only entities that
    `canBeCollidedWith` (boats, minecarts, shulkers) (5.8).

Confirmations of note: the mob effect intervals (`25 >> amp`, `40 >> amp`, `50 >> amp`), the
instant formulas and the undead inversion; the local cap as inferred (with: a player with nothing
counted passes at once); the spawnable chunks (Chebyshev 8 of a tracked player's chunk); the
custom spawners' order; every SpawnPlacements predicate; the synched ids of arrows and tridents;
`setBaseDamageFromMob`; the bow's power curve and velocity; the throws' gravities, rolls, powers
and sounds; creative abilities invulnerable; `NO_CREATIVE_OR_SPECTATOR`.

---------------------------------------------------------------------------------------

## 1. Arrows, tridents, fangs

Sources: `n9-javap/` AbstractArrow (AA), AbstractArrow$Pickup, Arrow, SpectralArrow,
ThrownTrident (TT), EvokerFangs, FireworkRocketEntity; the base from `n7k-javap/`
Projectile, ProjectileDeflection, ProjectileUtil; Enderman from `n9-javap/`. Line numbers are
of the `javap` file named. "Own" = the projectile's `random` (field #544 in AA).

### 1.1 What the bytecode says, in full

**Constants** (n9 AA 3–29): `ARROW_BASE_DAMAGE 2.0d`, `SHAKE_TIME 7`, `WATER_INERTIA 0.6f`,
`INERTIA 0.99f`, `FLAG_CRIT 1`, `FLAG_NOPHYSICS 2`. `getDefaultGravity` 0.05d (754–757),
`getAirDrag` 0.99f (567–570), `getWaterInertia` 0.6f (1959–1962). TT: `getWaterInertia` 0.99f
(n9 TT 7, 569–572).

**Synched data** (n9 AA 204–224, static 2047–2061; Projectile defines none: its method list,
n7k Projectile.txt, has no `defineSynchedData`; Entity 0–7):
- 8 `ID_FLAGS` byte, default 0 (bit 1 crit, bit 2 no physics; `setFlag` 1860–1892).
- 9 `PIERCE_LEVEL` byte, default 0.
- 10 `IN_GROUND` boolean, default false.
- Arrow: 11 `ID_EFFECT_COLOR` int, default −1 (n9 Arrow 120–131, 340–346).
- ThrownTrident: 11 `ID_LOYALTY` byte 0, 12 `ID_FOIL` boolean false (n9 TT 86–103, 593–603).
- (Firework, for reference: 8 item stack, 9 `DATA_ATTACHED_TO_TARGET` optional unsigned int,
  10 `DATA_SHOT_AT_ANGLE` boolean; n9 FireworkRocketEntity 176–195, 940–950.)

**Constructors** (n9 AA 53–165):
- `(type, level)`: pickup **DISALLOWED**, shake 0, life 0, baseDamage 2.0, soundEvent =
  `getDefaultHitGroundSoundEvent()` (ARROW_HIT; TT: TRIDENT_HIT_GROUND, n9 TT 474–477),
  pickup stack = default item, weapon null.
- `(type, x, y, z, level, pickupStack, weapon)`: pickup stack = copy; components applied;
  **`INTANGIBLE_PROJECTILE` removed from the given stack, and if it was there pickup =
  CREATIVE_ONLY**; `setPos`; if weapon non-null on a server: empty weapon throws, else
  `firedFromWeapon = weapon.copy()`, `pierce = EnchantmentHelper.getPiercingCount(level,
  weapon, pickupStack)` set if > 0.
- `(type, owner, level, pickupStack, weapon)`: at `(owner.x, owner.eyeY −
  0.10000000149011612, owner.z)`, then `setOwner(owner)`.
- `setOwner` (1709–1749): Player → `pickup == DISALLOWED ? ALLOWED : pickup` (so
  CREATIVE_ONLY set by the constructor stays); `OminousItemSpawner` → DISALLOWED; anything else
  (a mob) → unchanged, i.e. DISALLOWED.
- `shoot(...)` = `Projectile.shoot` then `life = 0` (226–238). `lerpMotion` also zeroes life
  and clears in-ground when the new motion is non-zero (240–259).
- Pickup ordinals: DISALLOWED 0, ALLOWED 1, CREATIVE_ONLY 2 (n9 AbstractArrow$Pickup 81–96).
- Trident `(level, owner, stack)` passes **weapon null** (n9 TT 26–53): no `firedFromWeapon`,
  so no piercing and no `doKnockback` knockback (which reads the field, n9 AA 1254–1256);
  `getWeaponItem()` is overridden to the pickup stack (n9 TT 436–440) for damage and
  enchantment effects. Loyalty = `clamp(getTridentReturnToOwnerAcceleration(level, stack,
  this), 0, 127)` (527–548), foil = `stack.hasFoil()`.

**`AbstractArrow.tick`** (n9 AA 284–565), in order:
1. `physics = !isNoPhysics()`; `v = getDeltaMovement()` (kept for the whole tick);
   `pos = blockPosition()`, `state` there.
2. If `state` not air and physics: its collision shape non-empty → for each AABB of the
   shape moved to `pos`, if it contains `position()` → **motion = ZERO, in-ground = true**,
   stop looking (296–345).
3. `shakeTime > 0` → `shakeTime--` (346–354).
4. `isInWaterOrRain()` → `clearFire()` (355–362).
5. **In ground and physics** (365–404): server: `lastState != state` (reference compare)
   **and** `shouldFall()` → `startFalling()`, else `tickDespawn()`; then `inGroundTime++`;
   alive → `applyEffectsFromBlocks()`; server: shared on-fire flag = remaining fire ticks >
   0; **return — `Projectile.tick` (so `Entity.tick`/`baseTick`) is not called in the
   ground** (fire ticks do not count down; a burning arrow in the ground burns until water
   or rain).
6. **Flying** (405–565): `inGroundTime = 0`; `p = position()`; `isInWater()` →
   `applyInertia(getWaterInertia())` (motion × 0.6, trident 0.99) **before the move** (and
   bubble particles); crit → 4 crit particles (no server draw); yaw target =
   `atan2(v.x, v.z)·57.2957763671875` (no physics: `atan2(−v.x, −v.z)`), pitch target =
   `atan2(v.y, |v.xz|)·57.29…`, both through `lerpRotation`; `checkLeftOwner()`;
   physics → `clipIncludingBorder(p → p + v, COLLIDER, fluid NONE)` and
   **`stepMoveAndHit(blockHit)`**; no physics → `setPos(p + v)`, `applyEffectsFromBlocks()`;
   then **not in water → `applyInertia(0.99)`**; then **physics and not in ground →
   `applyGravity()`** (−0.05); finally **`Projectile.tick()`** (first-tick
   `projectile_shoot` game event, `checkLeftOwner`, `Entity.tick` = baseTick) — the base tick
   runs **after** the move.
   The move uses `v`, the motion read at step 1, not the motion after the water inertia; so
   in effect: move by `v`, then motion = `v·0.6` in water or `v·0.99` in air, then gravity.
   `isInWater()` is the same flag both times (no baseTick between).
- `shouldFall` (759–780): in ground and `level.noCollision(AABB(pos, pos).inflate(0.06))`.
- `startFalling` (782–815): in-ground false; **motion × (nextFloat()·0.2f, nextFloat()·0.2f,
  nextFloat()·0.2f)** — three own draws in x, y, z order, each float product widened;
  `life = 0`. Also reached from `move(type ≠ SELF)` when `shouldFall` (847–861: a piston or
  shulker pushing it).
- `tickDespawn` (863–877): `++life >= 1200` → discard. **TT overrides** (550–567): no
  despawn at all while `pickup == ALLOWED && loyalty > 0`; otherwise the parent's 1200.

**`stepMoveAndHit(blockHit)`** (n9 AA 572–664), a loop while alive:
1. `start = position()`; `hits = findHitEntities(start, blockHit.location)` sorted by
   `start.distanceToSqr(entity.position())` (the entity's position, not the hit point;
   λ 2038–2045).
2. `end = hits.first.location` or the block hit's location; `setPos(end)`;
   `applyEffectsFromBlocks(start, end)`; a portal this tick → `handlePortal()`.
3. No entity hits: alive and the block hit not MISS → `hitTargetOrDeflectSelf(blockHit)`,
   `needsSync`; **return**.
4. Entity hits, alive and not `noPhysics`: `d = hitTargetsOrDeflectSelf(hits)` — each hit in
   order through `hitTargetOrDeflectSelf`, stopping at the first that returns a deflection
   other than NONE or leaves the arrow dead (666–692); `needsSync`; **pierce > 0 and d ==
   NONE → loop again from the new position** (pierced entities are excluded by
   `canHitEntity`); otherwise return. With entities hit, the block behind them is not hit in
   that step unless the loop comes round with no entity left.
- **`findHitEntities`** (1484–1503) = `ProjectileUtil.getManyEntityHitResult(level, this,
  from, to, box.expandTowards(motion).inflate(1), canHitEntity, false, false)`: **every**
  entity whose box the segment clips, else whose box inflated by `computeMargin` it clips with
  the clip from there to the box's centre (stopped by a block) still entering the real box;
  the hit location is the inflated box's entry point (n7k ProjectileUtil.txt 444–586).
  `findHitEntity` (single nearest, `getEntityHitResult`, 1465–1482) is **used only by the
  trident**, whose `findHitEntities` returns that one or none, and none once `dealtDamage`
  (n9 TT 270–296).
- **`canHitEntity`** (1505–1544): a Player target of a Player owner who `!canHarmPlayer`
  → false; then `Projectile.canHitEntity` (`canBeHitByProjectile`, and not a passenger of the
  owner's vehicle before it has left the owner, n7k Projectile.txt 844–866) and not in
  `piercingIgnoreEntityIds`.
- **`hitTargetOrDeflectSelf`** (n7k Projectile.txt 613–692): the entity's `deflection` first
  (NONE but for `#deflects_projectiles`); a world-border block hit → REVERSE ×0.2 (arrows
  `shouldBounceOnWorldBorder` true, n9 AA 2026–2029); else `onHit` (the `projectile_land`
  game event after `onHitEntity`/`onHitBlock`).

**`onHitEntity`** (n9 AA 938–1250), in order:
1. `Projectile.onHitEntity` (nothing). `speed = (float) |motion|`; `dmg = baseDamage`;
   `source = arrow(this, owner ?? this)`; a weapon on a server → `dmg =
   EnchantmentHelper.modifyDamage(level, weapon, target, source, (float) dmg)` (Power).
2. **`j = Mth.ceil(Mth.clamp((double) speed · dmg, 0, 2.147483647E9))`**.
3. **Piercing** (pierce > 0): create the id set (5) and the killed list; if the set's size ≥
   `pierce + 1` → **discard, return** (no damage); else add the target's id.
4. **Crit**: `j = (int) min(own nextInt(j / 2 + 2) + j, 2147483647)` (integer division;
   drawn only for a crit arrow, before the hurt).
5. Owner a LivingEntity → `owner.setLastHurtMob(target)` (before the hurt, whatever its
   outcome).
6. `fire = target.getRemainingFireTicks()`; **the arrow on fire → `target.igniteForSeconds(5f)`**
   — no type test (see 1.2 on the enderman).
7. **`target.hurtOrSimulate(source, (float) j)`**:
   - **took**: a living target: server and pierce ≤ 0 → arrow count + 1 (**piercing arrows
     never stick**); `doKnockback`; `doPostAttackEffectsWithItemSource` (weapon);
     `doPostHurtEffects` (Arrow: its potion contents; Spectral: glowing); a Player target, a
     ServerPlayer owner, not silent, target ≠ owner → `ClientboundGameEventPacket(
     PLAY_ARROW_HIT_SOUND, 0)` to the owner; killed tracking (advancement). Then, for any
     target, **`playSound(soundEvent, 1.0, 1.2f / (own nextFloat()·0.2f + 0.9f))`**; pierce ≤
     0 → discard.
   - **refused**: only if `target.projectileReceivesSideEffectsOnHit(false)` (true for every
     entity, `Entity` n7 returns 1; the **enderman returns its argument, so false**, n9
     Enderman 973–): `target.setRemainingFireTicks(fire)` (the ignition undone);
     **`deflect(REVERSE, target, owner, false, 0.2)`** → REVERSE with scale (0.2, 0.2, 0.2):
     one own `nextFloat()` f, **motion × (−0.5·0.2) = × −0.1** on each axis, **yaw and yRotO
     += 170 + 20f**, owner re-set to the same reference (n7k Projectile.txt 698–735;
     ProjectileDeflection.txt λ$static$1); then server: **motion² < 1.0E-7** → ALLOWED drops
     its pickup item (`spawnAtLocation(…, 0.1f)`) and in every case discard. No sound.
   - Neither branch for a refused hurt on the enderman: the arrow stays alive, unbounced, and
     goes on (the loop of 1.1 hits the next entity in the list).
- `doKnockback` (1252–1315): `k = firedFromWeapon != null && server ? modifyKnockback(level,
  weapon, target, source, 0f) : 0`; k > 0 → `r = max(0, 1 − knockback_resistance)`, `push =
  (motion.x, 0, motion.z).normalize() · (k · 0.6 · r)`; non-zero → `target.push(push.x, 0.1,
  push.z)`. No draws; nothing without Punch.

**`onHitBlock`** (n9 AA 1317–1408): `lastState = state at the hit`; `Projectile.onHitBlock`
(`state.onProjectileHit`); a weapon on a server → `hitBlockEnchantmentEffects`; **position −=
sign(motion) · 0.05000000074505806** (the position is already the hit point from the step);
motion ZERO; **`playSound(getHitGroundSoundEvent(), 1.0, 1.2f / (own nextFloat()·0.2f +
0.9f))`**; in-ground true; shake 7; crit off; pierce 0; `soundEvent = ARROW_HIT`; pierced lists
cleared.

**Pick-up** (n9 AA 1751–1806): `playerTouch`: server, (in ground **or** no physics) and
`shakeTime <= 0`, then `tryPickup`: DISALLOWED false; ALLOWED `inventory.add(pickupItem copy)`;
CREATIVE_ONLY `player.hasInfiniteMaterials()` — **any** player with infinite materials, not
only the shooter; success → `player.take(this, 1)`, discard. `isPickable` = Projectile's and
not in ground (1998–2009); `push`, bubble columns ignored in the ground (902–936).
- TT `playerTouch` (n9 TT 479–491): only by its owner, or anyone when it has no owner.
  `tryPickup` (442–464): the parent's, or (no physics, owned by the player, `inventory.add`).

**Save keys** (n9 AA 1546–1707): `life` short, `inBlockState` (nullable block state),
`shake` byte, `inGround` boolean, `pickup` (legacy codec, default DISALLOWED), `damage` double
(default 2.0), `crit` boolean, `PierceLevel` byte, `SoundEvent` (registry name), `item`
(stack), `weapon` (nullable stack); plus Projectile's `Owner`, `LeftOwner`, `HasBeenShot`,
`can_break`. SpectralArrow `Duration` int (200). TT `DealtDamage` boolean (loyalty re-read from
the item on load). Fangs `Warmup`, `Owner`.

**`setBaseDamageFromMob(power)`** (n9 AA 1937–1957): `baseDamage = (double)(power · 2f) +
own random.triangle(difficultyId · 0.11, 0.57425)`.

**Arrow** (n9 Arrow): its potion is the pickup stack's `potion_contents` component (47–55),
scaled by the stack's `potion_duration_scale` (default 1.0, 57–67). `addEffect` (110–118,
what the stray, bogged and parched call) **adds the effect to that component and updates
the colour**, so a mob's effect arrow is a coloured tipped arrow on the wire (data 11).
Colour = −1 for empty contents, else `PotionContents.getColor()` (90–108). `doPostHurtEffects`
(232–252): `contents.forEachEffect(e -> target.addEffect(e, effectSource), scale)`. Server
`tick` after the parent's (133–183): in ground, `inGroundTime != 0`, contents not empty and
`inGroundTime >= 600` → entity event **0** (the client's 20 coloured particles,
`handleEntityEvent` 262–329) and the pickup stack becomes a plain arrow (colour −1). No draws on
the server (the particles are client-side).

**SpectralArrow** (n9 SpectralArrow): `doPostHurtEffects` adds `GLOWING`, duration `duration`
(200), amplifier 0, source `getEffectSource()` (77–96); particles client-only; default item
spectral arrow.

**ThrownTrident tick** (n9 TT 105–235), before the parent's tick:
1. `inGroundTime > 4` → `dealtDamage = true` (so from the tick that begins with 5 ticks in
   the ground).
2. `loyalty > 0` and (`dealtDamage` or no physics) and an owner:
   - owner not acceptable (dead, or a spectator `ServerPlayer`, 237–258) → server: ALLOWED
     drops its item (0.1), discard;
   - an owner that is not a Player within `ownerBbWidth + 1` of `position→owner eye` →
     discard, return (a mob catches it);
   - else `setNoPhysics(true)`; `d = owner.eyePosition − position`; `setPosRaw(x, y + d.y ·
     0.015 · loyalty, z)`; motion = `motion · 0.95 + d.normalize() · (0.05 · loyalty)`;
     `trident_return` at volume 10, pitch 1 when `clientSideReturnTridentTickCount == 0`
     (counted on the server too: once); count + 1.
3. `AbstractArrow.tick()`. With no physics: no hit test, `setPos(p + v)`, the 0.99 drag (or
   0.99 water inertia), **no gravity**.
- **`onHitEntity`** (298–399), **not** the parent's (no speed scaling, no crit, no arrow
  count, no `setLastHurtMob`, no fire): `dmg = 8.0f`; `source = trident(this, owner ??
  this)`; server → `modifyDamage(level, getWeaponItem(), target, source, 8f)`; **`dealtDamage
  = true` before the hurt**; `took = hurtOrSimulate(source, dmg)`; took → server
  `doPostAttackEffectsWithItemSourceOnBreak`, a living target `doKnockback` (always 0: no
  weapon field) and `doPostHurtEffects` (nothing); then **if
  `projectileReceivesSideEffectsOnHit(took)`** (true for all but a refusing enderman) →
  `trident_hit` at 1.0, 1.0 (**no draw**) and **`deflect(REVERSE, target, owner, false,
  (0.02, 0.2, 0.02))`**: one own `nextFloat()`, motion × (−0.01, −0.1, −0.01), yaw +170–190.
  **The trident bounces this way whether or not the hurt took.**

**EvokerFangs** (n9 EvokerFangs): `lifeTicks = 22`, warmup from the constructor; yaw =
`yawRadians · 57.295776f` (25–46). Server tick (n9 EvokerFangs 109–280, the server branch from bytecode offset 224): after
`Entity.tick`, `--warmup`; while `warmup < 0`: at `warmup == −8` every `LivingEntity` in
`getBoundingBox().inflate(0.2, 0, 0.2)` gets `dealDamageTo`; then, once, entity event **4**;
then `--lifeTicks < 0` → discard. `dealDamageTo` (281–336): skip a dead target, one with
`isInvulnerable()` (the Invulnerable flag), or the owner; no owner → `hurt(magic, 6f)`; an ally
of the owner → skip; else `hurtServer(indirectMagic(this, owner), 6f)` and, if it took,
`doPostAttackEffects`. Its own `hurtServer` returns false. Server draws: none (the particle and
sound draws are in client branches).

**FireworkRocketEntity** (brief only infers its use): constructor at a position (35–104):
`flight = 1 + fireworks.flightDuration` (1 without the component); motion =
`(triangle(0, 0.002297), 0.05, triangle(0, 0.002297))`; **`lifetime = 10·flight +
nextInt(6) + nextInt(7)`** (own draws, in that order after the two triangles). Tick (226–543):
`Projectile.tick` first; not shot at an angle → motion × (h, 1, h) + (0, 0.04, 0) with h = 1.0
after a horizontal collision else 1.15; the hit test on the move vector, `move(SELF)`, the
motion restored; a hit → `hitTargetOrDeflectSelf`; `life == 0` and not silent → launch sound
(3.0, 1.0); `life++`; `life > lifetime` → explode (entity event 17, `explode` game event,
damage, discard). Damage (573–722): `5 + 2·explosions` (0 without explosions → nothing),
attached entity first, then living entities within 5 (box inflate 5, distance² ≤ 25) with a
clear line to their feet or half height, `dmg · sqrt((5 − dist)/5)`. Hitting an entity
explodes; a block explodes only with explosions.

### 1.2 The brief's claims, settled

| # | Brief (section, line) | Bytecode | Verdict |
|---|---|---|---|
| 1 | 0 f.3, 62–64: a refused arrow "bounces off (REVERSE deflection, the motion × 0.2)" | REVERSE with scale 0.2: **motion × −0.1** (−0.5 × 0.2) per axis, yaw +170 + 20·f, one own `nextFloat`; fire ticks restored; discarded (dropped if ALLOWED) only when motion² < 1e-7 after (n9 AA 1206–1250; n7k ProjectileDeflection λ$static$1, Projectile 698–735) | **CORRECTED** |
| 2 | 0 f.3: a trident that hits them bounces off | TT bounces by REVERSE (0.02, 0.2, 0.02) **whether or not the hurt took**, one own draw (n9 TT 376–399) | CONFIRMED, made exact |
| 3 | 0 f.15, 148–151: move before the drag, gravity 0.05, water 0.6, stuck test first, three draws to fall, a draw per hit sound, crit `nextInt(j/2+2)`, the piercing loop | all as stated (n9 AA 284–565, 782–815, 1192–1199, 1036–1055, 572–664). Additions: water inertia is applied before the move but the move uses the start-of-tick motion; the in-ground branch returns without `Projectile.tick`/baseTick; baseTick runs after the move | CONFIRMED (with the additions) |
| 4 | 0 f.16, 152–155: refused → "reverses with one own draw, slows to 0.2 and may drop" | reverses to a **tenth** (× −0.1), yaw turned 170–190; drops only below speed² 1e-7 (practically only an arrow already almost still), and only ALLOWED; discarded then in any case. Enderman: no side effects, the arrow flies on | **CORRECTED** |
| 5 | 3.2, 451–453: `setBaseDamageFromMob(power)` = `power · 2 + triangle(diffId · 0.11, 0.57425)`, two draws | `(double)(power · 2f) + random.triangle(id · 0.11, 0.57425)` (n9 AA 1937–1957); one `triangle` call (two `nextDouble` by the interface's default, not in this dump) | CONFIRMED |
| 6 | 3.5, 533–535: fangs 0.5 × 0.8; at warmup −8 hurt living in box inflate (0.2, 0, 0.2) but owner and allies by `indirectMagic` 6; event 4 once; gone after 22 ticks of life; no server draws | damage as stated, also skipping dead and `isInvulnerable()` targets, and an ownerless fang hurts by plain `magic` 6 (n9 EvokerFangs 281–336). Event 4 at the **first** tick of negative warmup (warmup −1), seven ticks before the damage. `lifeTicks` 22 counts down from that tick and the fang is discarded when it falls below 0: **23 ticks after the warmup ran out** (spawn + warmup + 23). Size: in `EntityType`, not dumped | damage CONFIRMED; timing **CORRECTED**; size STILL UNKNOWN |
| 7 | 4.1, 605: "`getManyEntityHitResult` is the spear's, not the arrow's" | `AbstractArrow.findHitEntities` **is** `getManyEntityHitResult(…, false, false)`: all entities along the step, sorted by distance from the start to each entity's position; the arrow hits them in that order (stopping when it dies or is deflected) (n9 AA 1484–1503, 572–692) | **CORRECTED** |
| 8 | 4.2, 613–616: eye − 0.1; ALLOWED for a player owner, DISALLOWED for a mob; CREATIVE_ONLY for intangible ammo; base 2.0; pierce from the weapon | as stated; CREATIVE_ONLY wins over ALLOWED (set before `setOwner`, which only upgrades DISALLOWED); pierce read only with a weapon on a server; an `OminousItemSpawner` owner → DISALLOWED (n9 AA 53–165, 1709–1749). Whether a creative bow makes intangible ammo is `ProjectileWeaponItem`'s (part 3) | CONFIRMED |
| 9 | 4.2, 617–621: flags byte 8 (crit 1, no physics 2), pierce byte 9, in-ground boolean 10; Arrow colour int 11 (−1); trident loyalty byte 11, foil boolean 12 | exactly (n9 AA 204–224, 2047–2061; Arrow 120–131; TT 86–103) | CONFIRMED |
| 10 | 4.2, 622–630: tick: stuck test; shake; fire out; in ground: block changed and `shouldFall` (inflate 0.06) → three draws × 0.2, life 0, else `++life >= 1200`; flying: water 0.6 (trident 0.99, "Pumpkin says 0.9"), rotation, clip, `stepMoveAndHit`, then 0.99 unless in water, then gravity | all as stated; **trident water inertia 0.99f** (n9 TT 7, 569–572); the "block changed" test is a reference comparison with the state saved at the last block hit; gravity skipped if the step put it in the ground; the in-ground branch does `inGroundTime++`, block effects and the fire flag, and no baseTick | CONFIRMED (trident 0.99 settled) |
| 11 | 4.2, 631–637: `onHitEntity` | the formula, crit draw and owner's `setLastHurtMob` as stated. Refinements: the arrow count rises only for **pierce ≤ 0**; the hit sound is the arrow's `soundEvent` (default `arrow_hit`, a crossbow's arrow `crossbow_hit`), played for **any** target the hurt took on, living or not; a player owner hitting a player gets game event 6. The fire: **no enderman test** in the arrow (the brief's "not an enderman"); the enderman refuses the hurt (`IS_PROJECTILE` → teleport, false) and, receiving no side effects, keeps the 5 s of fire the arrow gave it before the hurt. Refused: × −0.1, not × 0.2 (row 1) | **CORRECTED** (refusal, enderman); rest CONFIRMED |
| 12 | 4.2, 638–639: `onHitBlock` | as stated; the offset is `0.05000000074505806` (0.05f), from the step's hit position; also the sound event reset to `arrow_hit` after playing, the pierced lists cleared, and `lastState` saved before the block's own hook | CONFIRMED |
| 13 | 4.2, 640–643: pick-up; "a creative player's arrows can be picked up by them, and add nothing" | `playerTouch`/`tryPickup` as stated (n9 AA 1751–1806). A CREATIVE_ONLY arrow can be picked up by **any** player with infinite materials, not only its shooter; it adds nothing. An ALLOWED arrow goes through `Inventory.add` (what that does in creative is not in these dumps) | CONFIRMED, broadened |
| 14 | 4.3, 649–652: trident damage 8, `dealtDamage`, motion × (−0.01, −0.1, −0.01), `trident_hit`; no arrow count; nothing more after damage or after 4 ticks in ground; loyalty returns it; picked up only by its owner or ownerless; a drowned's DISALLOWED, 1200 ticks | all as stated (n9 TT 105–235, 270–399, 479–491, 550–567). Exact: 8.0f through `modifyDamage`, **no speed factor**; `dealtDamage` set before the hurt (so a refused hit still ends its hitting); the bounce is REVERSE (one own draw, yaw 170–190) and happens even when the hurt took; `inGroundTime > 4`; the return: `y += d.y·0.015·L`, motion `·0.95 + d̂·0.05·L`, no gravity, `trident_return` once; a mob owner catches it within `width + 1`; with loyalty and ALLOWED it never despawns | CONFIRMED, made exact |
| 15 | 9, 985: arrow in flight: own 3 when it falls, 1 per hit sound, `nextInt(j/2+2)` crit, 1 at a deflection | order within a hit: crit `nextInt` (crit only, before the hurt) → took: 1 `nextFloat` (sound) / refused: 1 `nextFloat` (REVERSE) / enderman refusal: none; block hit 1 `nextFloat` (sound); fall 3 `nextFloat`. Trident: 1 `nextFloat` per bounce, no sound draw | CONFIRMED, ordered |
| 16 | 10, 1009–1010: arrows data 8, 9, 10 (11 colour), position and rotation always, `TakeItemEntity`, owner in the add packet; tridents 8–10, 11, 12 | confirmed; add: entity event **0** when a tipped arrow has lain 600 ticks in the ground; `ClientboundGameEventPacket` type 6 (arrow hit) to a player who hit a player; a mob's effect arrow (stray, bogged, parched) carries its colour at 11, since `addEffect` writes the potion component | CONFIRMED, with additions |
| 17 | 15.1 (lingering) / 4.4 not this part; fireworks in 15.1–15.2 "rockets of no flight" | without the `fireworks` component `flight = 1`: a rocket still flies, `lifetime = 10 + nextInt(6) + nextInt(7)` ticks, rising (×1.15 horizontal, +0.04 y a tick), and explodes harmlessly (no explosions → no damage) (n9 FireworkRocketEntity 35–104, 226–543, 573–722) | **CORRECTED** (a rocket without components flies about 10–21 ticks) |

### 1.3 Other details that bear on the plan

- A burning arrow sets a target alight **before** its hurt and undoes it on a refusal: across
  a line the holder must apply the fire only with the hurt's outcome (finding 16 / question 4).
- A refused, non-enderman hit leaves the arrow alive and slowed by ten, close to the target;
  the next tick it falls back. With question 4's option (b) the bounce disappears across a
  line, as the brief says; within a region it is a sharp, visible event (an arrow at a creative
  player).
- Piercing arrows hit every entity along a step in one tick (sorted by the entities'
  positions), and never stick in them.
- The arrow's base tick (fire countdown, water state) runs after the move, and not at all in
  the ground: Clustine's thrown tick should not be reused unchanged for arrows.

---------------------------------------------------------------------------------------

## 2. Thrown items and the cloud

Files are in `/home/claude/reference/n9-javap/`; "n9 X lines a–b" are line numbers of that
class's `javap` file (`cat -n`). Item components quoted as "data" are from
`target/datagen/26.3/generated/reports/minecraft/components/item/*.json` and potion tags from
`generated/data/minecraft/tags/potion/`.

### 2.1 Gravity of every throwable (brief 4.4 table, Gravity column)

| Projectile | Bytecode | Verdict |
|---|---|---|
| egg | no `getDefaultGravity` in ThrownEgg; inherits `ThrowableProjectile` 0.03 (n7k ThrowableProjectile.txt 205–208) | CONFIRMED 0.03 |
| ender pearl | no override; 0.03 | CONFIRMED 0.03 |
| splash and lingering | n9 AbstractThrownPotion lines 39–42: `0.05d` | CONFIRMED 0.05 |
| experience bottle | n9 ThrownExperienceBottle lines 38–41: `0.07d` | CONFIRMED 0.07 |

### 2.2 The throws from a player's hand (brief 4.4 "Use" column, 9 row "player throws")

All by `Projectile.spawnProjectileFromRotation(factory, level, stack, player, roll, power,
uncertainty)` (n7k Projectile.txt 498–513: create, then `spawnProjectile`: the consumer
`shootFromRotation` (six own draws), `addFreshEntity`, `applyOnProjectileSpawned`, 557–568), then
`awardStat`, `stack.consume(1, player)`, `SUCCESS`. The sound is played **before** the projectile
is created, volume 0.5, pitch `0.4f / (L.nextFloat() · 0.4f + 0.8f)` with L the level's random
(one draw, always, on the server too).

| Item | Roll, power, uncertainty | Sound, source | Cite | Verdict |
|---|---|---|---|---|
| egg (all three) | 0, 1.5, 1.0 | `egg_throw`, PLAYERS | n9 EggItem lines 12–64 | CONFIRMED |
| snowball | 0, 1.5, 1.0 | `snowball_throw`, NEUTRAL | n9 SnowballItem lines 12–64 | CONFIRMED (the inferred throw Clustine built) |
| splash potion | −20, 0.5, 1.0 | `splash_potion_throw`, PLAYERS, played in `SplashPotionItem.use` before `super.use` | n9 SplashPotionItem lines 10–38; ThrowablePotionItem lines 12–44 | CONFIRMED |
| lingering potion | −20, 0.5, 1.0 | `lingering_potion_throw`, NEUTRAL | n9 LingeringPotionItem lines 10–38 | CONFIRMED |
| experience bottle | −20, 0.7, 1.0 | `experience_bottle_throw`, NEUTRAL | n9 ExperienceBottleItem lines 10–62 | CONFIRMED |
| ender pearl | 0, 1.5, 1.0 | `ender_pearl_throw`, NEUTRAL | n9 EnderpearlItem lines 12–64 | CONFIRMED |

- **Cooldowns**: none of these `use` methods sets a cooldown. The ender pearl's 20 ticks is the
  item's `use_cooldown` component, `{seconds: 1.0}` (data, `ender_pearl.json`), applied by the
  generic item-use path, not by `EnderpearlItem`. The egg, snowball, potions and bottle have no
  `use_cooldown`. **CONFIRMED (20 ticks), but from data, not bytecode**; Clustine needs the
  generic `use_cooldown` handling (and `ClientboundCooldown`) rather than a pearl special case.
- `ThrowablePotionItem.PROJECTILE_SHOOT_POWER = 0.5f`, `EggItem`/`SnowballItem`/`EnderpearlItem`
  `1.5f` (constants at the top of each file).
- Stack sizes (data): egg and snowball 16, ender pearl 16, splash and lingering potion 1,
  experience bottle 64.

### 2.3 Dispenser configurations (brief 4.6, **Inferred** defaults)

- **`DispenseConfig$Builder` defaults** (n9 ProjectileItem$DispenseConfig$Builder lines 11–27):
  uncertainty `6.0f`, power `1.1f`, no override event, position function
  `DispenserBlock.getDispensePosition(source, 0.7, new Vec3(0, 0.1, 0))` (lines 78–89).
  `getDispensePosition(source, d, offset)` is `centre + (d · stepX + offset.x, d · stepY +
  offset.y, d · stepZ + offset.z)` (n6 DispenserBlock.txt 312–330).
  **CORRECTED**: the projectile starts at the dispenser's centre + 0.7 · facing **+ 0.1 in y**,
  not centre + 0.7 · facing as the brief has it (that is the one-argument overload, 304–310,
  used by item dropping). Uncertainty 6.0 and power 1.1: CONFIRMED.
- `ProjectileItem.createDispenseConfig` default is `DispenseConfig.DEFAULT` = the builder's
  defaults (n9 ProjectileItem lines 5–8; ProjectileItem$DispenseConfig lines 81–86): eggs,
  snowballs, arrows (plain, tipped, spectral: `ArrowItem` has no override) use 6.0 / 1.1.
- Potions (n9 ThrowablePotionItem lines 59–73) and the experience bottle (n9
  ExperienceBottleItem lines 79–93): uncertainty `DEFAULT.uncertainty · 0.5f` = 3.0, power
  `DEFAULT.power · 1.25f` = `1.1f · 1.25f` = 1.375 (as a float, exactly 1.375f). **CONFIRMED.**
- Firework rocket (n9 FireworkRocketItem lines 154–189): its own position function
  `centre + step · 0.5000099999997474` (no y offset), uncertainty 1.0, power 0.5, override event
  1004. **CONFIRMED.**
- Tridents and ender pearls: `TridentItem` **is** a `ProjectileItem` (n9 TridentItem line 2,
  `asProjectile` 307–327, ALLOWED pickup), but `DispenseItemBehavior.bootStrap` registers the
  projectile behaviour only for arrow, tipped arrow, spectral arrow, the three eggs, snowball,
  experience bottle, splash, lingering, firework, fire charge and wind charge (n6
  DispenseItemBehavior.txt 11–35). `EnderpearlItem` is not a `ProjectileItem` (n9 line 2).
  Finding 18's "tridents and ender pearls are dropped": **CONFIRMED.**
- A dispensed arrow (n9 ArrowItem lines 23–44, SpectralArrowItem 23–44): `new Arrow(level, x,
  y, z, stack.copyWithCount(1), null)`, `pickup = ALLOWED`, no owner. **CONFIRMED.**

### 2.4 The potions' hit (brief 4.4 rows splash and lingering; finding 3; section 11 "a splash")

**`AbstractThrownPotion.onHit`** (n9 AbstractThrownPotion lines 104–188), server only, after
`ThrowableItemProjectile.onHit` (which ran `onHitEntity`/`onHitBlock`):

1. `contents = stack.getOrDefault(POTION_CONTENTS, EMPTY)`.
2. **`affectEntitiesAround(level, contents)`** — always, whatever the potion (below).
3. If `contents.hasEffects()`: `onHitAsPotion(level, stack, hit)` (the splash's effects, or
   the lingering cloud). **A water bottle has no effects, so a lingering water bottle makes no
   cloud.**
4. Level events at `blockPosition()`: if `contents.potion()` is present **and that base potion
   `hasInstantEffects()`** → `2007` with `contents.getColor()`, then unless silent `1054` with
   data 0; otherwise → `2002` with the colour, then unless silent `1053` with data 0. Custom
   effects never select 2007 (only the base potion is asked). The sound is now a separate level
   event (1053/1054), which the brief does not name. **CORRECTED (detail added).**
5. `discard()`.
No draws anywhere in the hit (server side).

**`affectEntitiesAround`** (lines 190–286): the water behaviour, by potion tags (all four tags
hold only `minecraft:water`, data):
- `box = getBoundingBox().inflate(4, 2, 4)` — the potion's own box where it is now, **not**
  moved to the hit point.
- If the potion is in `hurts_water_sensitive_entities` or `extinguishes_entities`: every
  `LivingEntity` in the box that `isSensitiveToWater() || isOnFire()` (predicate, 372–383) with
  **centre distance** `distanceToSqr(entity) < 16`: if `hurts…` and sensitive →
  `hurtServer(level, indirectMagic(this, owner), 1.0f)`; if `extinguishes…` and on fire and
  alive → `extinguishFire()`.
- If in `rehydrates_axolotls`: every `Axolotl` in the same box, **no distance test**, →
  `rehydrate()`.

**`onHitBlock`** (lines 44–102), server only: if the potion is in `douses_fire`: with `p =
hitBlock.relative(face)`: `douseFire(p)`, `douseFire(p.relative(face.opposite))` (the hit block
itself), then `douseFire` on p's four horizontal neighbours. `douseFire` (290–347): a block in
`#fire` → `destroyBlock(pos, false, this)`; a lit candle → `AbstractCandleBlock.extinguish(null,
…)`; a lit campfire → level event 1009, `CampfireBlock.douse(owner, …)`, `LIT = false`.

**`ThrownSplashPotion.onHitAsPotion`** (n9 ThrownSplashPotion lines 38–180) — **CORRECTED** in
how the distance is measured:
- `scale = stack.getOrDefault(POTION_DURATION_SCALE, 1.0f)` (a splash potion has none: 1.0,
  data).
- `moved = getBoundingBox().move(hit.getLocation() − position())` — the potion's box carried to
  the hit point; `area = moved.inflate(4, 2, 4)`; `targets = getEntitiesOfClass(LivingEntity,
  area)` (no predicate; the thrower included).
- `margin = ProjectileUtil.computeMargin(this)` (float, n7k ProjectileUtil 363–377).
- For each target with `isAffectedByPotions()` (alive: n7d LivingEntity.txt 10241–10249;
  the armour stand overrides; `Player` does not, so a creative player is affected):
  `d2 = moved.distanceToSqr(target.getBoundingBox().inflate(margin))` — **box to box**, so a
  direct hit or any box touching gives `d2 = 0`; if `d2 < 16`: `prox = 1 − sqrt(d2) / 4`.
- For each effect of `contents.getAllEffects()` (the base potion's, then the custom ones):
  instantaneous → `effect.applyInstantaneousEffect(level, this, getOwner(), target, amp, prox)`;
  otherwise `dur = (int)(prox · (double)duration · (double)scale + 0.5)` (lambda 182–194, by
  `mapDuration`, so an infinite duration stays infinite), `new MobEffectInstance(effect, dur,
  amp, ambient, visible)` (the icon flag not carried), added with
  `addEffect(instance, getEffectSource())` **unless `instance.endsWithin(20)`**.
- The brief's "every living entity in `inflate(4, 2, 4)` with d² < 16, `prox = 1 − √d / 4`" is
  right in form, but d is the distance between the moved potion box and the target's box
  inflated by the margin, not between centres. A golem beside a creative player: each gets its
  own proximity.

**`ThrownLingeringPotion.onHitAsPotion`** (n9 ThrownLingeringPotion lines 38–114):
`new AreaEffectCloud(level, x, y, z)` at **the hit entity's position** for an entity hit, else
at the potion's position; owner if a `LivingEntity`; `setRadius(3.0f)`, `setRadiusOnUse(−0.5f)`,
`setDuration(600)`, `setWaitTime(10)`, `setRadiusPerTick(−radius / duration)` = −0.005f;
`applyComponentsFromItemStack(stack)` (potion contents and `potion_duration_scale`, 0.25 for a
lingering potion, data); `addFreshEntity`. Brief's "radius 3, −0.5 a use, 600 ticks, wait 10":
**CONFIRMED**, with the per-tick shrink and the 0.25 duration scale added.

### 2.5 `AreaEffectCloud` (n9 AreaEffectCloud)

- **Fields and defaults** (lines 3–99): `TIME_BETWEEN_APPLICATIONS = 5`; `duration = −1`
  (infinite), `waitTime = 20`, `reapplicationDelay = 20`, `durationOnUse = 0`, `radiusOnUse =
  0`, `radiusPerTick = 0`, `potionDurationScale = 1.0`, default radius 3.0, `MAX_RADIUS 32`,
  minimum 0.5, `noPhysics = true`; `victims` a `HashMap<Entity, Integer>` (only filtered by
  `removeIf`, so its order never matters). The lingering potion leaves `reapplicationDelay` at
  20 and `durationOnUse` at 0.
- **Synched data** (lines 114–134, static 1057–1076): `DATA_RADIUS` FLOAT (default 3.0),
  `DATA_WAITING` BOOLEAN (false), `DATA_PARTICLE` PARTICLE (`entity_effect` with colour −1 by
  default; `updateParticle` sets it from the custom particle or the potion's colour, 207–232).
  Defined in this order directly on `Entity`'s 0–7, so **8, 9, 10** (**Inferred** from the
  order; the ids are not constants in the bytecode).
- **Size**: `EntityDimensions.scalable(radius · 2, 0.5)` (950–958); `setRadius` clamps to
  [0, 32] (135–151). `hurtServer` always false (960–963).
- **`serverTick`** (453–704), after `Entity.tick` (286–306):
  1. `duration != −1 && tickCount − waitTime >= duration` → discard, return.
  2. `waiting = tickCount < waitTime`; set the synched flag if it changed; waiting → return.
  3. `r = radius`; if `radiusPerTick != 0`: `r += radiusPerTick`; `r < 0.5` → discard,
     return; `setRadius(r)`.
  4. Only when `tickCount % 5 == 0`: drop victims whose stored tick `<= tickCount`; no effects
     → clear victims, done; else `effects` = `contents.forEachEffect(list::add,
     potionDurationScale)` (each `withScaledDuration(scale)`, PotionContents lines 152–197);
     for each `LivingEntity` in `getBoundingBox()` (in the level's order): skip a victim, one
     not `isAffectedByPotions`, or one for which no effect passes a bound-method predicate
     `noneMatch(target::…)` (**the method is not named in `-c` output**: presumably
     `canBeAffected`; STILL UNKNOWN); horizontal `dx² + dz² <= (double)(r · r)` (the float
     square widened) → `victims[target] = tickCount + reapplicationDelay`; each effect:
     instantaneous → `applyInstantaneousEffect(level, this, owner, target, amp, 0.5)`, else
     `target.addEffect(new MobEffectInstance(effect), this)`; then if `radiusOnUse != 0`: `r +=
     radiusOnUse`, `< 0.5` → discard and return, else `setRadius(r)`; if `durationOnUse != 0`
     and the duration is finite: `duration += durationOnUse`, `<= 0` → discard, return.
  - **No server draws** (the only `random` calls are in `clientTick`, 307–452).
  - Note that `r` is a local carried through the loop, so a second target in the same pass is
    tested against the already shrunk radius.
- **Save keys** (775–931): `Age` (tickCount), `Duration`, `WaitTime`, `ReapplicationDelay`,
  `DurationOnUse`, `RadiusOnUse`, `RadiusPerTick`, `Radius`, `Owner`, `custom_particle`,
  `potion_contents`, `potion_duration_scale`.
- Cost for N9 (question 15.1 (c)): one entity type with three synched values, no draws, an
  application every 5 ticks over its box; across a line the box (up to 6 wide at radius 3)
  reaches into copies, so an entity effect per target is needed, as for the splash.

### 2.6 The egg (brief 4.4 row egg; 9 row "egg hit")

- **`onHitEntity`** (n9 ThrownEgg lines 96–111): parent's, then `entity.hurt(thrown(this,
  owner), 0.0f)` (the `Entity.hurt` path, not `hurtServer`). CONFIRMED.
- **`onHit`** (lines 113–195), server only, after the parent's:
  1. own `nextInt(8)`; if `== 0`: `n = 1`, then own `nextInt(32)`; if `== 0`: `n = 4`.
     (The second draw only when the first was 0.)
  2. `n` times: `CHICKEN.create(level, TRIGGERED)`; null → skip; `setAge(−24000)`;
     `snapTo(egg x, y, z, egg yaw, 0)`; the stack's `CHICKEN_VARIANT` component if present →
     set on the chick (absent: the chicken constructor's default; no `finalizeSpawn`, so **no
     level draws and no biome variant**); `fudgePositionAfterSizeChange(ZERO_SIZED_DIMENSIONS)`
     — **if it fails, the loop stops: no further chicks** (goto 168); else `addFreshEntity`.
  3. Entity event 3, `discard()`.
  CONFIRMED, with two additions: the chick takes the egg's yaw, and a failed fudge ends the
  loop. Each chick's construction draws its own UUID (as every entity).

### 2.7 The experience bottle (brief 4.4 row; 9)

`onHit` (n9 ThrownExperienceBottle lines 43–112), server only, after the parent's:
1. level event `2002` at `blockPosition()` with data `−13083194` (= `PotionContents.
   BASE_POTION_COLOR`), then unless silent level event `1053`;
2. `xp = 3 + own nextInt(5) + own nextInt(5)` — **two own draws, always**;
3. a block hit → `ExperienceOrb.awardWithDirection(level, hit.getLocation(),
   face.getUnitVec3(), xp)`; otherwise (an entity hit) → `awardWithDirection(level,
   hit.getLocation(), motion · −1, xp)`;
4. `discard()`.
**CORRECTED (detail)**: `3 + nextInt(5) + nextInt(5)` is the total experience, not the number of
orbs; it is split into orbs by `awardWithDirection` (not in this dump; it may draw), at the hit
point with a direction.

### 2.8 The ender pearl (brief 4.4 row; 1 in 15)

**`onHit`** (n9 ThrownEnderpearl lines 137–318), after the parent's:
1. **32 iterations of `level.addParticle(PORTAL, x, y + own nextDouble() · 2, z, own
   nextGaussian(), 0, own nextGaussian())`** — the arguments are evaluated on the server too, so
   **96 own draws (nextDouble, nextGaussian, nextGaussian per iteration) on every hit**, before
   anything else. **New; not in the brief's draw table.**
2. Not a server level, or already removed → return.
3. `owner = getOwner()`; null or `!isAllowedToTeleportOwner(owner, level)` (320–350: same
   dimension → a living owner must be alive and not sleeping, another entity alive; otherwise
   `canUsePortal(true)`) → discard, return.
4. `pos = oldPosition()` (the pearl's position before this tick's move; the exact field behind
   `oldPosition` is `Entity`'s and not in this dump — **ambiguous** whether that is the hit
   point or the previous tick's position; the hit has already moved it).
5. **A `ServerPlayer` owner**: only if its connection `isAcceptingMessages()` (else straight to
   discard):
   - own `nextFloat() < 0.05f` (**drawn every time**), **and** `isSpawningMonsters()`, and the
     difficulty not peaceful → `ENDERMITE.create(level, TRIGGERED)` at the **player's**
     position and rotation (before the teleport), `addFreshEntity` (no `finalizeSpawn`);
   - if the pearl `isOnPortalCooldown()` → the owner's `setPortalCooldown()`;
   - `player.teleport(new TeleportTransition(level, pos, Vec3.ZERO, 0, 0, Relative.ROTATION,
     DO_NOTHING))` (keeps the player's rotation, zero motion); if it returned a player:
     `resetFallDistance`, `resetCurrentImpulseContext`, `hurtServer(level, enderPearl(),
     5.0f)` (a creative player refuses it as invulnerable, damage-type permitting);
   - `playSound(level, pos)`: `player_teleport`, PLAYERS, volume and pitch 1 (454–467: the
     two-float-less overload, no draw).
6. **Any other owner**: `owner.teleport(new TeleportTransition(level, pos, ZERO, owner yaw,
   owner pitch, DO_NOTHING))`, then `resetFallDistance` and (a living one)
   `resetCurrentImpulseContext`; no damage; the sound.
7. `discard()`.
- `onHitEntity` (120–135): `entity.hurt(thrown(this, owner), 0)`.
- **`tick`** (352–452): server: the owner by UUID in any dimension or the player list; a dead
  `ServerPlayer` owner that has not won the game with `ender_pearls_vanish_on_death` → discard;
  else the parent's tick; then the chunk ticket timer (`registerAndUpdateEnderPearlTicket`)
  when it runs out or the pearl changed chunk. `onRemoval` deregisters from the owner unless
  `UNLOADED_WITH_PLAYER` (the pearl is saved with the player). End gateways pass the owner
  through.
- Brief's "an endermite one in 20 (**Inferred**)": **CONFIRMED** (own `nextFloat() < 0.05f`),
  with the conditions above; **CORRECTED** in the draws: 96 particle draws first, and the 0.05
  draw happens for a player owner whatever the difficulty.

---------------------------------------------------------------------------------------

## 3. Weapons in hand and the mobs' shots

### 3.1 Use durations (brief 3.1 line 411, **Inferred**)

| Item | `getUseDuration` | Verdict |
|---|---|---|
| bow | 72000 (n9 BowItem lines 160–163) | CONFIRMED |
| trident | 72000 (n9 TridentItem lines 56–59) | CONFIRMED |
| crossbow | **72000** (n9 CrossbowItem lines 570–574) | **CORRECTED**: not "charge + 3". The charge is measured against `getChargeDuration` inside `onUseTick`, and the crossbow loads there (3.4). |

`getChargeDuration(stack, entity)` = `Mth.floor(EnchantmentHelper.modifyCrossbowChargingTime(
stack, entity, 1.25f) · 20)` = 25 without Quick Charge (lines 575–587). **CONFIRMED 25.**

### 3.2 `ProjectileWeaponItem` (n9 ProjectileWeaponItem)

- `ARROW_ONLY` = `#arrows`; `ARROW_OR_FIREWORK` adds the firework rocket (lines 339–354).
  `getHeldProjectile(entity, pred)`: the **off hand first**, then the main hand, else EMPTY
  (22–45).
- **`draw(weapon, ammo, shooter)`** (193–254): empty ammo → empty list; `n =
  processProjectileCount(level, weapon, shooter, 1)` (Multishot; 1 without); for `i` in 0..n:
  `useAmmo(weapon, i == 0 ? ammo : ammo.copy(), shooter, i > 0)`, non-empty results kept.
- **`useAmmo(weapon, ammo, shooter, isCopy)`** (256–317): `amount = (isCopy ||
  shooter.hasInfiniteMaterials() || client) ? 0 : processAmmoUse(level, weapon, ammo, 1)`;
  `amount > count` → EMPTY; `amount == 0` → `ammo.copyWithCount(1)` with
  **`INTANGIBLE_PROJECTILE`** set; else `ammo.split(amount)`, and a player whose stack emptied
  has it removed from the inventory. So a creative player's arrows and every multishot copy
  are intangible (which is what makes them CREATIVE_ONLY in the arrow: AbstractArrow's part).
  **CONFIRMED** (brief 4.2 "CREATIVE_ONLY when the bow drew intangible ammo").
- **`shoot(level, shooter, hand, weapon, projectiles, velocity, inaccuracy, crit, target)`**
  (49–200):
  - `spread = processProjectileSpread(level, weapon, shooter, 0)` (Multishot sets 10);
    `step = size == 1 ? 0 : 2 · spread / (size − 1)`; `offset = ((size − 1) % 2) · step / 2`;
    `sign = 1`;
  - for `i` in order, skipping empty stacks: `angle = offset + sign · ((i + 1) / 2) · step`
    (integer division), `sign = −sign`; `p = createProjectile(level, shooter, weapon, stack,
    crit)`; `Projectile.spawnProjectile(p, level, stack, consumer)` with the consumer
    `shootProjectile(shooter, p, i, velocity, inaccuracy, angle, target)` — so per projectile:
    **create (its construction's draws), shoot (six own draws), add, `onProjectileSpawned`**;
    then `weapon.hurtAndBreak(getDurabilityUse(stack), shooter, hand slot)`; a broken weapon
    ends the loop.
  - Three projectiles with spread 10: angles **0, −10, +10** (i = 0, 1, 2). **CONFIRMED**
    (brief 4.5 "at 0, −10, +10").
- **`createProjectile`** (162–191): the ammo's item if an `ArrowItem`, else `Items.ARROW`;
  `createArrow(level, ammo, shooter, weapon)`; `setCritArrow(true)` when `crit`. No
  `setBaseDamageFromMob` here.

### 3.3 The bow in a player's hand (brief 4.5 bow; 9 row "player bow")

- **`use`** (n9 BowItem lines 170–196): `has = !player.getProjectile(stack).isEmpty()`; if
  `!player.hasInfiniteMaterials() && !has` → FAIL; else `startUsingItem(hand)`, CONSUME. (A
  creative player can draw with no arrow at all.)
- **`releaseUsing(stack, level, entity, remaining)`** (14–116): only a `Player`; `ammo =
  player.getProjectile(stack)`, empty → false; `t = 72000 − remaining`; `f =
  getPowerForTime(t)`; `(double)f < 0.1` → false; `list = draw(stack, ammo, player)`; on the
  server with a non-empty list: `shoot(level, player, usedHand, stack, list, f · 3.0f, 1.0f,
  f == 1.0f, null)`; then (both sides; on the server always) `playSound(null, x, y, z,
  arrow_shoot, PLAYERS, 1.0, 1.0f / (L.nextFloat() · 0.4f + 1.2f) + f · 0.5f)` with L the
  level random — **after** the shot; `awardStat`; true.
- **`getPowerForTime(t)`** (134–158): `f = t / 20f; f = (f · f + f · 2) / 3; min(f, 1)`; 1.0 at
  20, 0.1075 at 3, 0.07 at 2: at least three ticks. **CONFIRMED** (and the skeleton's 1.0 at 20,
  brief 3.2: CONFIRMED).
- **`shootProjectile`** (118–132): `p.shootFromRotation(shooter, xRot, yRot + angle, 0,
  velocity, inaccuracy)`. **CONFIRMED** (`3f · f`, uncertainty 1.0, crit at full power).
- **Draw order for one arrow**: [the arrow's construction draws] → six own draws of the
  projectile (`shoot`) → level `nextFloat()` (sound). The bow adds no draw of its own; whether
  "the arrow's 2" of brief 4.5 exists is AbstractArrow's constructor (the bow does not call
  `setBaseDamageFromMob`, so no triangle draws from the bow path). **CORRECTED** if "2" meant the
  mob triangle; section 9's "arrow UUID + 6; level 1 (sound, after)" is the right form.
- `getDefaultProjectileRange` 15; `MAX_DRAW_DURATION` 20.

### 3.4 The crossbow (brief 3.3 and 4.5 crossbow)

- **State on the item singleton** (n9 CrossbowItem lines 3–21, 23–30): `startSoundPlayed` and
  `midLoadSoundPlayed` are **instance fields of the one `CrossbowItem`**, shared by every
  holder in the server. **CONFIRMED** (brief 3.3 line 489, **Inferred**). They are reset by a
  player's `use` (46–91) and by `onUseTick` whenever progress `< 0.2`.
- **`use`** (46–91): a non-empty `CHARGED_PROJECTILES` → `performShooting(level, player, hand,
  stack, getShootingPower(charged), 1.0f, null)`, CONSUME; else if `player.getProjectile(stack)`
  is not empty → both flags false, `startUsingItem`, CONSUME; else FAIL.
  `getShootingPower` (93–102): **1.6 if any charged item is a firework rocket, else 3.15** —
  one value for all charged projectiles. **CONFIRMED** (3.15 / 1.6).
- **`onUseTick(level, entity, stack, remaining)`** (489–569), server only:
  `progress = (72000 − remaining) / (float)chargeDuration`;
  - `< 0.2` → both flags false;
  - `>= 0.2` and not `startSoundPlayed` → set; `crossbow_loading_start` (or the enchantment's
    sound) at the entity, **PLAYERS**, volume 0.5, pitch 1.0 (lambda 683–701; no draw);
  - `>= 0.5` and not `midLoadSoundPlayed` → set; `crossbow_loading_middle`, PLAYERS, 0.5, 1.0
    (664–682; no draw);
  - `>= 1.0`, not yet charged, and `tryLoadProjectiles(entity, stack)` (128–149: `draw(stack,
    entity.getProjectile(stack), entity)`, non-empty → `CHARGED_PROJECTILES =
    ofNonEmpty(list)`) → `crossbow_loading_end` at the entity, **the entity's sound source**,
    volume 1.0, pitch `1.0f / (L.nextFloat() · 0.5f + 1.0f) + 0.2f` (634–663) — **one level
    draw when it loads**.
  - With `updateUsingItem` calling `onUseTick(remaining)` before the decrement (n7d
    LivingEntity.txt 9472–9490), the k-th use tick sees `72000 − remaining = k − 1`: the start
    sound on the 6th use tick, the middle on the 14th, and **the load on the 26th while the
    use is still held**. A player holding right-click sees the crossbow load without
    releasing.
- **`releaseUsing`** (104–126): returns `getPowerForTime(t) >= 1 && isCharged(stack)`; it
  **loads nothing**. `useOnRelease` is true for a crossbow (622–628), and
  `LivingEntity.releaseUsingItem` (n7d LivingEntity.txt 9942–9977) then runs
  **`updatingUsingItem()` once more** before `stopUsingItem()`: that extra `onUseTick` is what
  loads a pillager's crossbow. **CORRECTED** (brief 3.3 line 481 "`releaseUsingItem()` (the
  crossbow loads)" and 4.5 "at release with full power the projectiles stored"): the loading is
  in `onUseTick` at progress ≥ 1, reached at release only through the extra use tick. For the
  pillager the order is: the goal's tick sees `getTicksUsingItem() >= 25` (the 25th use tick,
  whose own `onUseTick` saw 24), calls `releaseUsingItem`, whose extra `onUseTick` sees 25,
  loads (mob's `getProjectile`, `draw`), plays the end sound (one level draw), then stops
  using; then the goal draws `nextInt(20)` for `attackDelay`. So **the end sound's level draw
  comes before the goal's `nextInt(20)` (own)**.
- **`performShooting(level, shooter, hand, stack, velocity, inaccuracy, target)`** (395–450):
  server only; `CHARGED_PROJECTILES` set to EMPTY (the old value taken); empty → return;
  `shoot(level, shooter, hand, stack, copies, velocity, inaccuracy, **shooter instanceof
  Player**, target)` — **crit for any player, never for a mob**; a `ServerPlayer` gets the
  `shot_crossbow` trigger and the stat. CONFIRMED (crit for a player).
- **`createProjectile`** (342–383): a firework → `new FireworkRocketEntity(level, ammo, shooter,
  x, eyeY − 0.15000000596046448, z, true)`; else the parent's arrow, then
  `setSoundEvent(crossbow_hit)` on an `AbstractArrow`. CONFIRMED.
- **`getDurabilityUse`** (384–394): 3 for a firework, else 1. CONFIRMED ("wears by 1").
- **`shootProjectile(shooter, p, index, velocity, inaccuracy, angle, target)`** (167–276):
  - with a target: `dx = tx − sx`, `dz = tz − sz`, `h = sqrt(dx² + dz²)`, `dy = target.getY(1/3)
    − p.getY() + h · 0.20000000298023224`; `v = getProjectileShotVector(shooter, (dx, dy, dz),
    angle)`;
  - without (a player): `up = shooter.getUpVector(1)`; `q = new Quaternionf().setAngleAxis(
    (double)(angle · 0.017453292f), up.x, up.y, up.z)`; `v = shooter.getViewVector(1)
    .toVector3f().rotate(q)`;
  - `p.shoot((double)v.x, (double)v.y, (double)v.z, velocity, inaccuracy)` (six own draws);
  - `pitch = getShotPitch(shooter.getRandom(), index)`; `playSound(null, shooter x, y, z,
    crossbow_shoot, shooter.getSoundSource(), 1.0, pitch)` — **a sound for every projectile**.
- **`getProjectileShotVector(shooter, dir, angle)`** (277–341), JOML floats: `d =
  dir.toVector3f().normalize()`; `axis = d × (0, 1, 0)`; if `|axis|² <= 1e−7` → `axis = d ×
  shooter.getUpVector(1)`; `perp = d.rotateAxis(1.5707964f, axis)`; return `d.rotateAxis(angle
  · 0.017453292f, perp)` (each on a copy of `d`).
- **`getShotPitch(random, index)`** (451–488): index 0 → 1.0 (**no draw**); odd → `1 /
  (R.nextFloat() · 0.5 + 1.8) + 0.63`; even ≥ 2 → `… + 0.43`; R the shooter's own random,
  drawn after that projectile's six. **CONFIRMED.**
- **Draws for one charged shot of n projectiles**: per projectile i: [its construction] → six
  of its own → (i > 0) one of the shooter's → then the next projectile. Then nothing.

### 3.5 The mob's crossbow shot (brief 3.3 lines 484–490, **Inferred**)

`CrossbowAttackMob.performCrossbowAttack(mob, velocity)` (n9 CrossbowAttackMob lines 9–44):
`hand = ProjectileUtil.getWeaponHoldingHand(mob, CROSSBOW)`; if that stack's item is a
`CrossbowItem`: `performShooting(level, mob, hand, stack, velocity,
rangedAttackUncertainty(level), getTarget())`; then **always** `onCrossbowAttackPerformed()`.
The pillager passes `velocity = 1.6f` (n7 Pillager.txt 349–355); uncertainty `14 − 4 · id`
(n7k RangedAttackMob.txt 5–15), 10 at easy.

| Brief claim | Bytecode | Verdict |
|---|---|---|
| `performShooting(…, 1.6, 14 − 4 · diffId, target)` | as above | CONFIRMED |
| one arrow at angle 0 | `draw` with Multishot count 1 → one; angle 0 | CONFIRMED |
| base damage 2.0, no mob triangle | `ProjectileWeaponItem.createProjectile` never calls `setBaseDamageFromMob` (n9 ProjectileWeaponItem 162–191); the 2.0 itself is AbstractArrow's | CONFIRMED (the "no triangle" part) |
| crit off | `crit = shooter instanceof Player` (CrossbowItem 72–78) | CONFIRMED |
| hit sound `crossbow_hit` | CrossbowItem 68–73 | CONFIRMED |
| aimed by JOML float rotations | 277–341 | CONFIRMED |
| `shoot` (six draws) | 150–173 | CONFIRMED |
| `crossbow_shoot` pitch 1.0 for the first | `getShotPitch(_, 0)` = 1.0, no draw; source the mob's (`HOSTILE`) | CONFIRMED |
| wears by 1 | `hurtAndBreak(1, …)` per projectile | CONFIRMED |
| `onCrossbowAttackPerformed` | called even if the held item is not a crossbow | CONFIRMED |
| the ammo | the mob's `getProjectile(crossbow)` at load time (Monster.txt 214–), through `draw`/`useAmmo`: a mob has no infinite materials, so `processAmmoUse` (1 without enchantments) and `split(1)` of a fresh stack: **not intangible** | new detail |

### 3.6 The trident in a player's hand (brief 4.3)

- Constants (n9 TridentItem lines 3–7): `THROW_THRESHOLD_TIME = 10`, `BASE_DAMAGE = 8.0f`,
  `PROJECTILE_SHOOT_POWER = 2.5f`; attributes +8 attack damage, −2.9 attack speed (16–38).
- **`use`** (279–305): `nextDamageWillBreak()` → FAIL; Riptide strength > 0 and not
  `isInWaterOrRain()` → FAIL; else `startUsingItem`, CONSUME.
- **`releaseUsing`** (61–277): only a `Player`; `t = 72000 − remaining < 10` → false; riptide
  `> 0` and (not in water or rain, or a passenger) → false; `nextDamageWillBreak` → false;
  `sound = pickHighestLevel(TRIDENT_SOUND)` or `trident_throw`; `awardStat`; on the server:
  `hurtWithoutBreaking(1, player)`; **no riptide**: `thrown = stack.consumeAndReturn(1, player)`
  (a creative player keeps theirs), `ThrownTrident` by `spawnProjectileFromRotation(…, player,
  0, 2.5f, 1.0f)`, `hasInfiniteMaterials()` → `pickup = CREATIVE_ONLY`, then
  `playSound(null, trident, sound, PLAYERS, 1, 1)` — **at the trident entity, no draw**; true.
  **Riptide** (s = `getTridentSpinAttackStrength`, from the enchantment): the look vector
  `(−sin yaw · cos pitch, −sin pitch, cos yaw · cos pitch)` (floats, `Mth.sin/cos`) normalised
  and scaled to length s, `player.push(…)`; `startAutoSpinAttack(20, 8.0f, stack)`; on the ground
  `move(SELF, (0, 1.1999999284744263, 0))`, the sound at the player; true.
  Brief 4.3: "released after at least 10 ticks of use, `consumeAndReturn`, shot at (0, 2.5,
  1.0), CREATIVE_ONLY for a creative player, then `trident_throw` with no draw":
  **CONFIRMED**; riptide confirmed as needing an enchantment (a component).
- Draws: [the trident's construction] → six own (`shootFromRotation`). No level draw.

### 3.7 Arrow items, the tipped arrow and charged projectiles

- `ArrowItem.createArrow(level, ammo, shooter, weapon)` = `new Arrow(level, shooter,
  ammo.copyWithCount(1), weapon)` (n9 ArrowItem lines 10–21); `SpectralArrowItem` the same with
  `SpectralArrow` (n9 SpectralArrowItem 10–21); `TippedArrowItem` only changes the default
  instance (`potion_contents` = poison, lines 10–25) and the name. The tipped arrow's
  `potion_duration_scale` is 0.125 (data).
- `ChargedProjectiles` (n9 ChargedProjectiles lines 3–16, 44–115): a list of
  `ItemStackTemplate`, at most 1024; `contains(item)`, `itemCopies()`, `ofNonEmpty`. The
  crossbow's default component is the empty list (data `crossbow.json`).
- `CrossbowItem$ChargeType` (NONE, ARROW, ROCKET) is the client's model predicate data; not
  used by the server's logic above.

### 3.8 The firework rocket item (for question 15.1 (b))

`FireworkRocketItem.use` (n9 lines 85–135): **only while fall flying** (else PASS): drop the
player's leashes (with `lead_break`), `new FireworkRocketEntity(level, stack, player)`,
`Projectile.spawnProjectile` (no shoot, so no six draws), `consume(1)`, stat. `useOn` a block
places a rocket at the clicked point offset 0.15 along the face (14–84). Firework entity
behaviour is in `FireworkRocketEntity` (dumped; not read for this part).

### 3.9 Corrections to the draw table (brief section 9)

| Row | Correction |
|---|---|
| egg hit | CONFIRMED: own `nextInt(8)`, then only on 0 own `nextInt(32)`; chicks draw only their UUIDs |
| player throws | CONFIRMED: level `nextFloat()` before the spawn, then the projectile's UUID and six |
| player bow | CONFIRMED form: arrow construction + six own, then level 1 after |
| (new) experience bottle hit | own `nextInt(5)`, own `nextInt(5)`, always; then the orbs' |
| (new) ender pearl hit | 96 own (32 × `nextDouble`, `nextGaussian`, `nextGaussian`), then for a player owner own `nextFloat()` (endermite) |
| (new) crossbow load | level `nextFloat()` (end sound) when it loads; the start and middle sounds draw nothing |
| (new) crossbow shot | per projectile: construction, six own, and for index ≥ 1 one of the shooter's own |
| (new) splash, lingering, cloud | no server draws |

---------------------------------------------------------------------------------------

## 4. Mob effects

Files: `net.minecraft.world.effect.*` in `n9-javap/`. "Interval" below is
`shouldApplyEffectTickThisTick(d, amp)`, where `d` is the remaining duration before the
countdown (brief section 5, MobEffectInstance.txt 456–500).

| Effect | `shouldApplyEffectTickThisTick(d, amp)` | `applyEffectTick(level, e, amp)` | Cite |
|---|---|---|---|
| poison | `k = 25 >> amp`; `k > 0 ? d % k == 0 : true` | if `getHealth() > 1.0f`: `hurtServer(level, magic, 1.0f)`; returns true | n9 PoisonMobEffect lines 13–48 (`DAMAGE_INTERVAL = 25`, line 3) |
| wither | `k = 40 >> amp`; same form | `hurtServer(level, wither, 1.0f)` with **no health test**; true | n9 WitherMobEffect lines 13–43 |
| regeneration | `k = 50 >> amp`; same form | if `getHealth() < getMaxHealth()`: `heal(1.0f)`; true | n9 RegenerationMobEffect lines 11–42 |
| hunger | always true (every tick) | a `Player` only: `causeFoodExhaustion(0.005f · (amp + 1))`; anything else nothing; true | n9 HungerMobEffect lines 11–33 |
| instantaneous (base of instant health and harming) | `d >= 1` | — | n9 InstantaneousMobEffect lines 11–24 (`isInstantaneous` true) |
| instant health / harming (`HealOrHarmMobEffect(isHarm)`) | inherits `d >= 1` | `isHarm == e.isInvertedHealAndHarm()` → `heal((float) max(4 << amp, 0))`; else `hurtServer(level, magic, (float)(6 << amp))`; true | n9 HealOrHarmMobEffect lines 16–45 |
| every other effect | `MobEffect` default **false** (never ticks) | default returns true | n9 MobEffect lines 101–118 |

- **The instant effect with a factor** (`applyInstantaneousEffect(level, source, owner, e, amp,
  f)`, n9 HealOrHarmMobEffect lines 46–101): heal branch (same `isHarm ==
  isInvertedHealAndHarm` test) `heal((float)(int)(f · (double)(4 << amp) + 0.5))`; harm branch
  `v = (int)(f · (double)(6 << amp) + 0.5)`, then `hurtServer(level, magic, v)` when `source ==
  null`, else `hurtServer(level, indirectMagic(source, owner), v)`. `f` is a `double`; the
  multiplication is in doubles; `(int)` truncates. The heal branch has no `max(…, 0)` here.
- `MobEffect.applyInstantaneousEffect`'s default (n9 MobEffect lines 106–113) ignores the factor
  and the source and calls `applyEffectTick(level, e, amp)`.
- **Undead inversion**: through `LivingEntity.isInvertedHealAndHarm()` (not in n9; brief section 5
  has the `#…` tags). Harming heals an inverted entity by `4 << amp`, health hurts it by `6 << amp`
  (magic, not indirect, in `applyEffectTick`).
- **Modifiers** (`MobEffect$AttributeTemplate.create(amp)`, n9 lines 24–40): amount × `(amp + 1)`
  in doubles, with the template's id and operation. Confirms the brief's "× (amp + 1)".
- Note on wither versus poison: wither hurts at any health (it can kill); poison stops at 1.0.

**Claims settled**

| Brief claim (section, line) | Bytecode | Verdict |
|---|---|---|
| 0 finding 21, line 188: poison `25 >> amp`, wither `40 >> amp`, regeneration `50 >> amp` | as above; a shift reaching 0 (amp ≥ 5 for poison, ≥ 6 for wither and regeneration) means every tick | CONFIRMED (with the 0 case) |
| 2.2, lines 316–318: hunger does nothing to a mob | only `Player` is touched | CONFIRMED |
| 5, lines 719–721: poison hurts 1 (magic) while health > 1; wither hurts 1 (wither); regeneration heals 1 | poison `getHealth() > 1.0f`; wither has no health test; regeneration only below max | CONFIRMED (wither unconditional, regeneration capped: additions) |
| 5, line 721: hunger every tick, exhaustion on a player | `0.005f · (amp + 1)` exhaustion | CONFIRMED (amount added) |
| 5, lines 722–723: instant `4 << amp` / `6 << amp` (magic), inverted for the undead, splash scaling `(int)(f · v + 0.5)` | as above; the splash's harm uses `indirectMagic(potion, owner)` when there is a source, `magic` only without | CONFIRMED, with a correction of detail: **a thrown potion's harm is `indirectMagic`, not `magic`** (it credits the thrower: `lastHurtByMob`, so a creeper hit by a witch's harming turns on the witch, and HurtBy alerting applies) |
| 5, line 724: modifiers × (amp + 1) | `amount · (amp + 1)` | CONFIRMED |

---------------------------------------------------------------------------------------

## 5. Spawning

### 5.1 The local cap (`LocalMobCapCalculator`)

n9 LocalMobCapCalculator lines 1–110 and `$MobCounts` (its own file, lines 1–55):

- State: `playersNearChunk: Long2ObjectMap<List<ServerPlayer>>` (a cache per chunk) and
  `playerMobCounts: Map<ServerPlayer, MobCounts>`; a new calculator is made with each spawn state
  (each tick, NaturalSpawner.createState, brief 8.2), so both live one tick.
- `getPlayersNear(chunk)` (lines 26–37): `computeIfAbsent` with
  `chunkMap.getPlayersCloseForSpawning(chunk)` — **computed once per chunk per tick and kept**.
- `addMob(chunk, category)` (lines 39–62): for every player in `getPlayersNear(chunk)`, that
  player's `MobCounts` (created empty if absent) `.add(category)` (+1; `$MobCounts` lines 18–27,
  43–54).
- `canSpawn(category, chunk)` (lines 64–94): for every player near the chunk: **a player with no
  `MobCounts` at all returns true at once**; otherwise true if `count(category) <
  category.getMaxInstancesPerChunk()` (the category's max, 70 for monsters: `$MobCounts` lines
  28–41). No player near → false.
- `getPlayersCloseForSpawning(chunk)` (n7 ChunkMap.txt 2228–2268): `distanceManager.
  hasPlayersNearby(chunk).toBoolean(true)` false → empty list; otherwise every player of
  `playerMap.getAllPlayers()` that passes `playerIsCloseEnoughForSpawning` (2270–2289: not a
  spectator, and `(cx·16 + 8 − px)² + (cz·16 + 8 − pz)² < 16384.0`, the chunk's centre from
  `SectionPos.sectionToBlockCoord(c, 8)`, 2313–2344). Horizontal, strictly below 128 blocks,
  creative players included.

So the brief's inference (8.2, lines 852–855; 0 finding 24, line 210) is **CONFIRMED** with three
precisions: (a) the count a player carries is of mobs in chunks whose centre is within 128 of
**that player**, collected at `createState` and raised by `afterSpawn` within the tick; (b) a
player with nothing counted yet passes immediately; (c) the near-players list of a chunk is
frozen for the tick at its first use.

### 5.2 The spawnable chunks and the 128-block test (`DistanceManager`, `ChunkTracker`)

- `naturalSpawnChunkCounter = new FixedPlayerDistanceChunkTracker(this, 8)` (n9 DistanceManager
  lines 15–26). The tracker (n9 DistanceManager$FixedPlayerDistanceChunkTracker lines 9–116): level
  0 at a chunk holding a tracked player (`getLevelFromSource`, 79–88; `havePlayer` reads
  `playersPerChunk`, 90–107), otherwise the minimum over the **eight neighbours and itself** + 1
  (n9 ChunkTracker lines 23–76, 78–141, 143–156: `dx, dz ∈ {−1, 0, 1}`), i.e. the Chebyshev
  distance in chunks to the nearest tracked player's chunk; `setLevel` keeps a chunk in `chunks`
  only while level ≤ `maxDistance` = 8 (49–73).
- `getNaturalSpawnChunkCount()` = `chunks.size()` after `runAllUpdates` (n9 DistanceManager lines
  386–395): every chunk within Chebyshev 8 of any tracked player's chunk, counted once, loaded or
  not. **CONFIRMED** (brief 8.2 lines 849–851).
- Who is tracked: `addPlayer(sectionPos, player)` (lines 226–265) puts the player in
  `playersPerChunk` and updates the counter; it is called from `ChunkMap.updatePlayerStatus`
  only when `!skipPlayer` (n7 ChunkMap.txt 2364–2418), and `skipPlayer` = spectator and
  `!spectators_generate_chunks` (2346–2362). So spectators count iff the game rule is true.
  **CONFIRMED** (brief 0 finding 24 line 208; the rule's default is in n7d GameRules, not re-read
  here).
- `hasPlayersNearby(chunk)` (lines 397–418): level ≤ `INSCRIBED_SQUARE_SPAWN_DISTANCE_CHUNK` → TRUE;
  level > 8 → FALSE; else DEFAULT. The constant is `Mth.floor(8.0f / Mth.SQRT_OF_TWO)` = 5 (n7
  NaturalSpawner.txt 1239–1243). TRUE is only a shortcut for the public
  `anyPlayerCloseEnoughForSpawning` (n7 ChunkMap 2152–2170); `getPlayersCloseForSpawning` and
  `collectSpawningChunks` still test the 128 blocks for each player. FALSE (beyond Chebyshev 8)
  gives no players.
- `getSpawnCandidateChunks()` (lines 450–460) = the key set of the same counter. `collectSpawning
  Chunks` (n7 ChunkMap 2105–2139) walks it, takes the holder's **ticking** chunk, and keeps it if
  any player passes the 128-block test. So the spawning chunks are: within Chebyshev 8 of a tracked
  player's chunk, ticking, and centre within 128 horizontally of a non-spectator player.
  **CONFIRMED** (8.2 lines 858–860), with the Chebyshev-8 prefilter added.
- The player ticket tracker is `PlayerTicketTracker(this, 32)` (line 30–40), unrelated to spawning.

### 5.3 The potential (`PotentialCalculator`)

n9 PotentialCalculator lines 1–63, `$PointCharge` 1–38: `addCharge(pos, c)` only if `c != 0`;
`getPotentialEnergyChange(pos, c)` = 0 when `c == 0`, else `c · Σ charge / sqrt(distSqr(pos,
chargePos))` (∞ at the same block). No draws. With no biome spawn costs in the overworld nothing is
charged. **CONFIRMED** (8.2 line 856).

### 5.4 Switches and the custom spawners (`MinecraftServer`, `Level`)

- `createLevels` (n9 MinecraftServer lines 587–, bytecode 59–98 at file lines 613–630) builds
  `ImmutableList.of(new PhantomSpawner(), new PatrolSpawner(), new CatSpawner(), new
  VillageSiege(), new WanderingTraderSpawner(savedDataStorage))` and passes it to the overworld's
  `ServerLevel` only; other dimensions get `ImmutableList.of()` (file line 777, bytecode 446).
  Order **CONFIRMED** (8.7 line 956): phantoms, patrols, cats, siege, wandering trader.
- `updateMobSpawningFlags()` (n9 MinecraftServer lines 2925–2943): for each level
  `setSpawnSettings(level.isSpawningMonsters())`; `Level.setSpawnSettings` passes it to the chunk
  source (n9 Level lines 1200–1208), which stores `spawnEnemies` (n7 ServerChunkCache.txt
  1249–1254). `isSpawningMonsters` = `spawn_mobs && spawn_monsters` (n7d ServerLevel.txt
  4187–4205). **CORRECTED** (8.1 line 815, "from `isSpawningMonsters` and not peaceful"):
  **`spawnEnemies` does not read the difficulty**; peaceful is refused per type in
  `SpawnPlacements.checkSpawnRules` (n9 SpawnPlacements lines 77–108: `!isAllowedInPeaceful &&
  PEACEFUL` → false, before the predicate). Callers at file lines 1081, 2911, 4733.
- **PhantomSpawner** (n9 PhantomSpawner lines 10–202), `tick(level, spawnEnemies)`:
  1. `!spawnEnemies` → return; `!spawn_phantoms` → return;
  2. `--nextTick > 0` → return; `nextTick += (60 + level nextInt(60)) · 20`;
  3. `getSkyDarken() < 5 && hasSkyLight` → return (day);
  4. for each player of `level.players()`: skip spectators; with sky light, skip if `y <
     seaLevel` or `!canSeeSky(pos)`; `diff = getCurrentDifficultyAt(pos)`; **level `nextFloat()`**:
     skip unless `diff.isHarderThan(nextFloat() · 3.0f)`; `t = clamp(stat TIME_SINCE_REST, 1,
     MAX)`; **level `nextInt(t)`**: skip unless `>= 72000`; position `pos.above(20 + nextInt(15))
     .east(−10 + nextInt(21)).south(−10 + nextInt(21))` (three level draws); skip unless
     `NaturalSpawner.isValidEmptySpawnBlock` for PHANTOM; count `1 + nextInt(diffId + 1)`; each:
     `PHANTOM.create(NATURAL)`, `snapTo(pos, 0, 0)`, `finalizeSpawn(level, diff, NATURAL,
     groupData)`, `addFreshEntityWithPassengers`.
  **No game-mode test but spectator**, and `TIME_SINCE_REST` is awarded every tick a player is not
  sleeping, in any game mode (n7d ServerPlayer.txt 1091–1102). **CORRECTED** (8.7 lines 962–963,
  "phantoms need insomnia (the player's time since rest: survival's statistic)"): **a creative
  player gets phantoms** after 72 000 ticks (an hour of play) without sleeping, at night, above
  sea level under open sky, at easy with chance `eff/3` per roll (25–50 %) and one or two
  phantoms. Phantoms then cannot target them (`asValidTarget`, brief finding 1) but circle.
- **PatrolSpawner** (n9 PatrolSpawner lines 9–285), `tick(level, spawnEnemies)`: `!spawnEnemies`
  or `!spawn_patrols` → return; `--nextTick > 0` → return; `nextTick += 12000 + level
  nextInt(1200)`; `!isBrightOutside()` → return (**patrols start in daylight**); level
  `nextInt(5) != 0` → return; no players → return; a player by level `nextInt(size)`; spectator →
  return; `isCloseToVillage(pos, 2)` → return; offsets `(24 + nextInt(24)) · (nextBoolean() ? −1
  : 1)` for x then z; `hasChunksAt(x±10, z±10)`; the environment attribute
  `CAN_PILLAGER_PATROL_SPAWN` at the position; group size `ceil(effectiveDifficulty) + 1`; members
  at the heightmap with `nextInt(5) − nextInt(5)` steps. **CORRECTED in mechanism** (8.7 line 964,
  "day 5"): there is no day test in the spawner; the `minecraft:early_game` timeline sets
  `can_pillager_patrol_spawn` false until tick 120000 of the overworld clock ("modifier": "and";
  `target/datagen/26.3/generated/data/minecraft/timeline/early_game.json`), and the mushroom fields
  biome sets it false. Effect: still from day 5. Interval 12000 + nextInt(1200) CONFIRMED.
- **VillageSiege** (n9 VillageSiege lines 28–113): bright or `!spawnEnemies` → SIEGE_DONE; at the
  overworld clock's `ROLL_VILLAGE_SIEGE` time marker one level `nextInt(10) == 0` → SIEGE_TONIGHT
  (drawn once a day whether or not there is a village); then a non-spectator player in a village
  (`isVillage`, points of interest), biome not `#without_zombie_sieges`, an angle `nextFloat() ·
  2π` at 32 blocks, 20 zombies. Needs points of interest (N10) except for that daily draw.

### 5.5 The slime (`Slime`, `AbstractCubeMob`, `WorldgenRandom`)

- **Class**: `monster.cubemob.Slime extends AbstractCubeMob extends AgeableMob` (n9 Slime line 2;
  AbstractCubeMob line 2). The sulfur cube is its sibling.
- **Synched data**: `AgeableMob` adds `DATA_BABY_ID` (boolean) and `AGE_LOCKED` (boolean) (n7
  AgeableMob.txt 648–660), so after Mob's 15 they are 16 and 17, and **`ID_SIZE` (int, default 1)
  is 18** (n9 AbstractCubeMob lines 76–88, 836–843). (Index by counting `defineId` up the
  hierarchy: Entity 0–7, LivingEntity 8–14, Mob 15.)
- **Save**: `Size` = size − 1 (int), `wasOnGround` (boolean) (n9 AbstractCubeMob lines 150–190),
  plus AgeableMob's keys.
- **Size**: `setSize(n, resetHealth)`: `n = clamp(n, 1, 127)`, data, reapply position, refresh
  dimensions, max health `n²`, speed base `0.2f + 0.1f · n` (float, then to double), health to max
  if asked (lines 89–138). `Slime.setSize` then sets attack damage base `n` and `xpReward = n`
  (n9 Slime lines 222–244).
- **`finalizeSpawn`** (n9 Slime lines 205–220; AbstractCubeMob lines 771–816): group data null →
  `AgeableMobGroupData(false)` (so AgeableMob's baby draw is skipped: n7 AgeableMob.txt 44–81 draws
  `nextFloat()` only when `isShouldSpawnBaby`); then AgeableMob → PathfinderMob/Mob `finalizeSpawn`
  (Mob's draws); then **`setSpawnSize`**: with `level.getRandom()`: `i = nextInt(3)`; if `i < 2`,
  `nextFloat() < 0.5f · specialMultiplier` → `i++` (the float drawn whenever `i < 2`, never
  passing at easy); size `1 << i` (1, 2 or 4); `setSize(size, true)`.
- **Goals**: 1 `CubeMobFloatGoal`; 2 `CubeMobAttackGoal` (Slime's `addBehaviourGoals`); 4
  `CubeMobRandomDirectionGoal`; 5 `CubeMobKeepOnJumpingGoal`; targets 1 `NAT(Player, 10, true,
  false, |target.y − y| <= 4.0)`, 3 `NAT(IronGolem, true)` (n9 AbstractCubeMob lines 40–70; Slime
  lines 11–47, 246–261). Own draws noted for the porter: the random direction re-rolls when
  `--nextRandomizeTime <= 0`: `adjustedTickDelay(40 + nextInt(60))`, then `nextInt(360)` (n9
  AbstractCubeMob$CubeMobRandomDirectionGoal lines 51–88); the float goal `nextFloat() < 0.8f`
  each tick; `getJumpDelay` `nextInt(20) + 10` (AbstractCubeMob 357–365); `getSplitCount` `2 +
  nextInt(3)` (537–546); other draws at lines 253–317 (tick), 409, 530, 617–620, 745–748 not read
  here.
- **`checkSlimeSpawnRules`** (n9 Slime lines 96–198), random = the level's:
  1. peaceful → false;
  2. `EntitySpawnReason.isSpawner(reason)` → `Mob.checkMobSpawnRules` (no draws there);
  3. **surface branch**: biome in `#allows_surface_slime_spawns` (swamp and mangrove swamp) and
     `50 < y < 70`: `chance = environmentAttributes().getValue(SURFACE_SLIME_SPAWN_CHANCE, pos)`;
     **`nextFloat() < chance`**, then **`getMaxLocalRawBrightness(pos) <= nextInt(8)`** (drawn only
     if the float passed), then `checkMobSpawnRules`; any failure falls through to 4 (not
     `return false`);
  4. **slime-chunk branch**: the level must be a `WorldGenLevel` (a `ServerLevel` is); `isSlimeChunk
     = seedSlimeChunk(cx, cz, level.getSeed(), 987234911L).nextInt(10) == 0`; then **level
     `nextInt(10)` drawn always** (whatever the chunk and the height); `== 0 && isSlimeChunk && y <
     40` → `checkMobSpawnRules`; else false.
  No `Monster` light draws: a slime never asks `isDarkEnoughToSpawn`.
- **The moon** for slimes: `SURFACE_SLIME_SPAWN_CHANCE` (default 0.0, range unit, n7d
  EnvironmentAttributes.txt 532–541) is set by the timeline `minecraft:moon` (overworld clock,
  period 192000, "modifier": "maximum", constant ease): 0.5 at full moon, 0.375, 0.25, 0.125, 0.0 at
  new moon, back up (`timeline/moon.json`), i.e. `0.5 · moonBrightness`.
- **`seedSlimeChunk(x, z, seed, salt)`** (n9 WorldgenRandom lines 180–210):
  `RandomSource.createThreadLocalInstance(seed + (long)(x·x·4987142) + (long)(x·5947611) +
  (long)(z·z)·4392871L + (long)(z·389711) ^ salt)` — the `int` products overflow as `int` before
  widening (`x·x·4987142`, `x·5947611`, `z·389711`; `z·z` is an int widened then multiplied as a
  long), and `^` binds after the sum. `createThreadLocalInstance` is `new
  SingleThreadedRandomSource(seed)` (n7 RandomSource.txt 36–42), the Java LCG.

Verdict on 8.4 line 926 (slime row): **CORRECTED**.
- The surface test is **one** draw against an environment attribute (`nextFloat() < 0.5 ·
  moonBrightness` in effect), not `nextFloat() < 0.5` then `nextFloat() < moon`; the biome test is
  the tag `allows_surface_slime_spawns`.
- The light test is `getMaxLocalRawBrightness` (darkened) `<= nextInt(8)`, drawn only after the
  float passes.
- A failed surface attempt **falls through** to the slime-chunk test.
- The slime-chunk test draws `nextInt(10)` on the level random **always**, before the chunk and
  the height are looked at; and the chunk's own `nextInt(10)` comes from a fresh seeded random (not
  a stream draw).
- `finalizeSpawn` draws `nextInt(3)` and, for size below 4, one `nextFloat()` (level), after Mob's
  draws.
- The slime is an `AgeableMob` (size at data 18, not 16).

### 5.6 The enderman (`Enderman` and its goals)

- Spawn: `ENDERMAN ON_GROUND MOTION_BLOCKING_NO_LEAVES Monster.checkMonsterSpawnRules` (table
  5.7). Synched: `DATA_CARRY_STATE` (optional block state) 16, `DATA_CREEPY` (boolean) 17,
  `DATA_STARED_AT` (boolean) 18 (n9 Enderman lines 235–258).
- Goals (lines 46–158): 0 float; 1 `EndermanFreezeWhenLookedAt`; 2 `MeleeAttackGoal(1.0, false)`;
  7 `WaterAvoidingRandomStrollGoal(1.0, 0f)`; 8 look at players 8; 8 look around; 10
  `EndermanLeaveBlockGoal`; 11 `EndermanTakeBlockGoal`. Targets: 1 `EndermanLookForPlayerGoal`
  (forCombat, range from the follow range, a selector; its tick uses 16 and 256 and 30, lines
  149–240); 2 `HurtByTargetGoal()`; 3 `NAT(Endermite, true, false)`; 4
  `ResetUniversalAngerTargetGoal(false)`.
- Take: `canUse` = no carried block, `mob_griefing`, own `nextInt(reducedTickDelay(20)) == 0`
  (`nextInt(10)`) (Enderman$EndermanTakeBlockGoal lines 14–44); tick: three `nextDouble` offsets
  (±2 x, 0–3 y, ±2 z), a clip, `#enderman_holdable` (45–). Leave: carried block, `mob_griefing`,
  own `nextInt(reducedTickDelay(2000))` = `nextInt(1000)` (Enderman$EndermanLeaveBlockGoal 14–44).
  With `mob_griefing` on, endermen move blocks within minutes: a player would notice.
- `customServerAiStep` (lines 482–527): bright outside and `tickCount >= targetChangeTime + 600`
  and `magic > 0.5f` and `canSeeSky(blockPosition)` and own `nextFloat() · 30f < (magic − 0.4f) ·
  2f` → target null, `teleport()`. `teleport()` (529–578): x `+ (nextDouble() − 0.5) · 64`, y `+
  nextInt(64) − 32`, z `+ (nextDouble() − 0.5) · 64`. It is `isSensitiveToWater` (477–481).
- `hurtServer` (lines 882–947): invulnerable → false; a hit whose direct entity is a thrown potion
  → `hurtWithCleanWater` (a `#hurts_water_sensitive_entities` potion hurts it); **any
  `#is_projectile` source while not a passenger → `repeatedlyTryToTeleport()` (up to 64 tries) and
  false**; otherwise the hurt, then if the attacker is not living, own `nextInt(10) != 0` →
  teleport. `projectileReceivesSideEffectsOnHit(b)` returns `b` (973–977). So arrows and tridents
  never hurt an enderman (section 1 has the arrow's side).
- `requiresCustomPersistence` (1008–): the parent's or a carried block.

### 5.7 `SpawnPlacements`: every predicate by index

From `SpawnPlacements-v.txt` `BootstrapMethods` (from file line 1507) and the static block (n9
SpawnPlacements lines 109–529). `checkSpawnRules` (lines 77–108): `!type.isAllowedInPeaceful() &&
difficulty == PEACEFUL` → false; a type with no entry → **true**; else its predicate.
`getPlacementType` defaults to `NO_RESTRICTIONS`, `getHeightmapType` to
`MOTION_BLOCKING_NO_LEAVES` (37–75).

| Index | Predicate | Types (placement; heightmap MOTION_BLOCKING_NO_LEAVES unless named) |
|---|---|---|
| #1 | `Axolotl.checkAxolotlSpawnRules` | axolotl (IN_WATER) |
| #2 | `WaterAnimal.checkSurfaceWaterAnimalSpawnRules` | cod, pufferfish, salmon (IN_WATER) |
| #3 | `AgeableWaterCreature.checkSurfaceAgeableWaterCreatureSpawnRules` | dolphin, squid (IN_WATER) |
| #4 | `Drowned.checkDrownedSpawnRules` | drowned (IN_WATER) |
| #5 | `Guardian.checkGuardianSpawnRules` | guardian, elder guardian (IN_WATER) |
| #6 | `TropicalFish.checkTropicalFishSpawnRules` | tropical fish (IN_WATER) |
| #7 | `Armadillo.checkArmadilloSpawnRules` | armadillo |
| #8 | `Bat.checkBatSpawnRules` | bat |
| #9 | `Monster.checkAnyLightMonsterSpawnRules` | blaze, breeze, zoglin |
| #10 | `Monster.checkMonsterSpawnRules` | bogged, cave spider, creeper, enderman, giant, skeleton, spider, witch, wither, wither skeleton, creaking, zombie, zombie horse, zombie villager, ravager (ON_GROUND); evoker, illusioner, vex, vindicator, warden (NO_RESTRICTIONS) |
| #11 | `Camel.checkCamelSpawnRules` | camel |
| #12 | `Monster.checkSurfaceMonstersSpawnRules` | camel husk, husk, parched |
| #13 | `Animal.checkAnimalSpawnRules` | chicken, cow, donkey, happy ghast, horse, llama, mule, pig, sheep, cat (ON_GROUND); panda, trader llama (NO_RESTRICTIONS) |
| #14 | `Endermite.checkEndermiteSpawnRules` | endermite |
| #15 | `Mob.checkMobSpawnRules` | ender dragon, iron golem, snow golem, villager, wandering trader (ON_GROUND); phantom, shulker (NO_RESTRICTIONS) |
| #16 | `Frog.checkFrogSpawnRules` | frog |
| #17 | `Ghast.checkGhastSpawnRules` | ghast |
| #18 | `GlowSquid.checkGlowSquidSpawnRules` | glow squid (IN_WATER) |
| #19 | `Goat.checkGoatSpawnRules` | goat |
| #20 | `MagmaCube.checkMagmaCubeSpawnRules` | magma cube |
| #21 | `SulfurCube.checkSulfurCubeSpawnRules` | sulfur cube (always true: n9 SulfurCube, the method's body is `iconst_1; ireturn`) |
| #22 | `MushroomCow.checkMushroomSpawnRules` | mooshroom |
| #23 | `AbstractNautilus.checkNautilusSpawnRules` | nautilus (IN_WATER) |
| #24 | `Ocelot.checkOcelotSpawnRules` | ocelot (heightmap MOTION_BLOCKING) |
| #25 | `Parrot.checkParrotSpawnRules` | parrot (heightmap MOTION_BLOCKING) |
| #26 | `Hoglin.checkHoglinSpawnRules` | hoglin |
| #27 | `Piglin.checkPiglinSpawnRules` | piglin |
| #28 | `PatrollingMonster.checkPatrollingMonsterSpawnRules` | pillager |
| #29 | `PolarBear.checkPolarBearSpawnRules` | polar bear |
| #30 | `Rabbit.checkRabbitSpawnRules` | rabbit |
| #31 | `Silverfish.checkSilverfishSpawnRules` | silverfish |
| #32 | `SkeletonHorse.checkSkeletonHorseSpawnRules` | skeleton horse |
| #33 | `Slime.checkSlimeSpawnRules` | slime |
| #34 | `Stray.checkStraySpawnRules` | stray |
| #35 | `Strider.checkStriderSpawnRules` | strider (IN_LAVA) |
| #36 | `Turtle.checkTurtleSpawnRules` | turtle |
| #37 | `Wolf.checkWolfSpawnRules` | wolf |
| #38 | `ZombifiedPiglin.checkZombifiedPiglinSpawnRules` | zombified piglin |
| #39 | `Fox.checkFoxSpawnRules` | fox (NO_RESTRICTIONS) |

(#0 is the "Duplicate registration" string concatenation of `register`.) Verdict on 8.4 lines
916–928: **CONFIRMED** for every type the brief's table names (the monster rule for zombie,
zombie villager, zombie horse, skeleton, bogged, creeper, spider, cave spider, enderman, witch,
evoker, vindicator, vex; surface monsters for husk, parched, camel husk; stray, drowned,
guardian and elder, slime, pillager by `PatrollingMonster`, animals); the brief's "which method
each index is, is Inferred" is now settled. Not in the brief's table: the **sulfur cube's rule is
`true`** (no light, no draws).

### 5.8 Smaller spawning claims

| Brief claim (section, line) | Bytecode | Verdict |
|---|---|---|
| 8.3 lines 866–867: `getHeight` is the heightmap's first free cell − 1 | `heightmaps.get(type)` (primed if missing) `.getFirstAvailable(x & 15, z & 15) − 1` (n9 ChunkAccess lines 286–330) | CONFIRMED |
| 8.3 line 885: `noCollision` covers entities and the border too | `noCollision(AABB)` = `noCollision(null, box, false)` = `noBlockCollision(null, box, false) && noEntityCollision(null, box) && noBorderCollision(null, box)` (n9 CollisionGetter lines 45–92); the entity part is `getEntityCollisions`, which takes only `NO_SPECTATORS.and(canBeCollidedWith(null))` (n5 EntityGetter.txt 86–115; n9 EntitySelector `CAN_BE_COLLIDED_WITH`) — hard-collision entities (boats, minecarts, shulkers), **not mobs** | CONFIRMED, narrowed: other mobs never block a spawn box |
| 8.3 line 892: `isUnobstructed` | `isUnobstructed(e)` = `isUnobstructed(e, Shapes.create(e.getBoundingBox()))` (n9 CollisionGetter lines 7–43) | as the brief |
| 8.3 table "piece" | `structureHasPieceAt(x, y, z, start)`: any of the start's pieces whose bounding box `isInside(x, y, z)` (n9 StructureManager lines 374–420) | CONFIRMED |
| 8.3 line 895: `Mob.finalizeSpawn` uses `level.getRandom()` | `Mob` not in n9; the slime's `setSpawnSize` uses `ServerLevelAccessor.getRandom()` and AgeableMob's baby draw does too (n7 AgeableMob 62–64) | STILL UNKNOWN for Mob's own (consistent) |
| 0 finding 26, line 220 and 8.5 line 939: sulfur caves list sulfur cube 100×2–4 | the sulfur cube's rule is `true`; Clustine has no sulfur cube type (`mobs/types.rs` has none), so in sulfur caves the cube's 100 weight ends positions as the slime's and enderman's do | addition to finding 26 |

### 5.9 The moon's brightness in 26.3

- For the difficulty: `ServerLevel.getMoonBrightness(pos)` = `DimensionType.MOON_BRIGHTNESS_PER_
  PHASE[environmentAttributes.getValue(MOON_PHASE, pos).index()]` (n7d ServerLevel.txt 1348–1365),
  the table `{1, 0.75, 0.5, 0.25, 0, 0.25, 0.5, 0.75}` (n7d DimensionType.txt 488–509), the phase
  from the `minecraft:moon` timeline's `visual/moon_phase` track (one phase per 24000 ticks of the
  overworld clock, period 192000).
- For slimes: not the brightness but `SURFACE_SLIME_SPAWN_CHANCE` from the same timeline (5.5).
- No n9 class reads `getMoonBrightness`.

---------------------------------------------------------------------------------------

## 6. Other

| Brief claim (section, line) | Bytecode | Verdict |
|---|---|---|
| 0 finding 1 line 41, 2.1, 14 line 1183: `NO_CREATIVE_OR_SPECTATOR` | `e instanceof Player p ? !e.isSpectator() && !p.isCreative() : true` (n9 EntitySelector lines 187–204); a non-player always passes | CONFIRMED |
| `NO_SPECTATORS` | `!e.isSpectator()` (lines 177–185) | CONFIRMED. (The pairing of each `InvokeDynamic` index in the static block, lines 248–269, with a `lambda$static$n` is not shown by `-c`; it is read from the bodies, which match the field names unambiguously: `CAN_BE_COLLIDED_WITH` = `NO_SPECTATORS.and(e.canBeCollidedWith(null))`.) |
| 0 finding 1 lines 42–44, 14 line 1184: `Avatar.canBeSeenAsEnemy` | `Avatar` (n9, lines 25–179) has **no** `canBeSeenAsEnemy`: only data, main arm, model parts, dimensions, profile. The player's is `Player`'s (n7d Player.txt 1603–1615, the brief's own citation) | CONFIRMED (nothing in between overrides it) |
| 0 finding 1 line 44, 14 line 1185: creative abilities are invulnerable | `GameType.updatePlayerAbilities` (n9 GameType lines 102–153): CREATIVE → mayfly, instabuild, **invulnerable** true (flying unchanged); SPECTATOR → mayfly, invulnerable, flying true, instabuild false; SURVIVAL and ADVENTURE → all four false; then `mayBuild = !isBlockPlacingRestricted()` (adventure or spectator) | CONFIRMED |
| 14 line 1185 (K 0.4, 7 line 795): does `enchantItemFromProvider` draw at a special multiplier of 0? | `enchantItemFromProvider(stack, access, key, difficulty, random)` (n9 EnchantmentHelper lines 1332–1351): looks up the provider; if present, `updateEnchantments(stack, m -> provider.enchant(stack, m, random, difficulty))` (1353–). **It tests no multiplier itself**; whether it draws is the provider's (`providers.*`, not dumped). `Mob.enchantSpawnedEquipment` guards it with own `nextFloat() < chance · specialMultiplier` (n7 Mob.txt 2615–2638), so at easy the provider is never reached from `finalizeSpawn` (the float is still drawn for a non-empty slot). Callers without that guard: the pillager, the vindicator, the skeleton trap (n7) and the enderman's loot drop (n9 Enderman lines 800–820, `ENDERMAN_LOOT_DROP` with its own random) | STILL UNKNOWN for the provider's own draws; settled for the mobs' spawn path |
| 14 line 1186: `Level.destroyBlockProgress` | abstract in `Level` (n9 Level line 1711); the body is `ServerLevel`'s | STILL UNKNOWN (not needed at easy) |
| 6 line 775–777: the skeleton trap only from thunder | `LightningBolt` (n9, lines 1–709) in brief: `life = 2`, own `nextLong()` seed and `flashes = nextInt(3) + 1` in the constructor (23–48); tick: at life 2 the client sounds; on the server, fire (`spawnFire(4)`) on normal and hard only, the lightning rod, copper cleaning, game event; life−−; below 0 with flashes 0 discard (and the advancement over entities within 15), else own `nextInt(10)` against life for another flash with a new seed and `spawnFire(0)`; while life ≥ 0 and not visual only, hurts the entities in `±3, +6, ±3`. `DAMAGE_RADIUS 3`, `DETECTION_RADIUS 15` | (no brief claim to settle; weather is out of N9) |
| 2.7 line 379: the vex's flags at 16 | `Vex` is not in n9 | STILL UNKNOWN (16 by counting: `Monster` adds none after Mob's 15) |
| 3.2 line 428, 0 finding 10 line 122, 1.2 line 277, 2.2 line 338, 6 line 756 | classes not in n9 (`GoalSelector`, the vex, the ravager, `SpearUseGoal`, the brain) | STILL UNKNOWN |

---------------------------------------------------------------------------------------

## Changes to the brief's recommendations (section 15)

Only the questions whose recommendation, or the reason given for it, a correction above changes.
The others (5, 8, 9, 10, 13, 15, 17, 18, 20) stand as written.

- **15.1 Which projectiles.** (c) becomes cheap: the lingering cloud is dumped, has three
  synched values, no server draws and one application pass every 5 ticks (2.5); its only cost is
  the per-target entity effect across a line, which the splash needs anyway. Recommend **(c)**.
  For the rockets in (b): a creative player cannot launch one from the hand except while
  gliding with elytra; what they do within minutes is place one on a block (`useOn`), and that
  rocket flies 10–21 ticks and pops without any component (3.8; 1.2 row 17). So rockets need the
  entity and the `useOn` path first, the `fireworks` component only for coloured, damaging
  explosions. The ender pearl's hit costs 96 own draws (2.8), which Clustine's keyed streams
  must reproduce even though nothing is shown from them.
- **15.2 Item components.** The case for (a) is stronger: `Arrow.addEffect` writes
  `potion_contents`, so even the stray's, bogged's and parched's arrows carry their effect and
  their colour (data 11) through the component (1.2 row 16), and the lingering cloud is built
  from `potion_contents` and `potion_duration_scale` (2.4). The set should include
  `potion_duration_scale` (lingering 0.25, tipped arrow 0.125: data) and `use_cooldown` (the
  pearl's 20 ticks are that component, 2.8). `fireworks` can wait (see 15.1).
- **15.3 Using an item.** Still (a), but the use state must call the item's **`onUseTick`** every
  use tick and honour **`useOnRelease`**: the crossbow loads in `onUseTick`, and a release runs
  one more use tick (3.4). A player charging a crossbow sees it load on the 26th tick while still
  holding the button; the use itself lasts 72000.
- **15.4 An arrow's hit across a line.** (b) still for arrows, with three facts added: a refused
  arrow lives on near the target at a tenth of its speed (so the seam at a line is an arrow that
  vanishes instead of dropping back, not one that "bounces off"); a burning arrow sets the fire
  **before** the hurt and the holder must undo it on a refusal; a piercing arrow hits several
  entities in one step, and (b) ends it at the first one across the line (to be named). **The
  trident needs none of this**: its fate does not depend on the hurt (it bounces either way and
  `dealtDamage` is set before), so it can stay in the sender's region with the hurt sent as an
  effect. The enderman refusal (no bounce, the arrow flies on) can be judged by the sender from
  the type alone.
- **15.6 Mob effects.** (a) stands; the intervals are now the bytecode's. One addition to the
  set: the harm of a thrown potion is `indirectMagic(potion, owner)`, so its hurt names the
  thrower (HurtBy targets and alerts follow, a witch's harming turns a creeper on the witch).
- **15.11 Which mobs.** (a) stands and grows: the **sulfur cube** is in the sulfur caves' monster
  list (100 × 2–4) with a rule that is always true (5.7), so without it those caves' positions end
  as the slime's do. Either the sulfur cube joins N9 with the slime (they share
  `AbstractCubeMob`, dumped, with its goals and move control) or its weight is named as a
  difference. The slime's rule needs the world seed and the `moon` timeline's
  `surface_slime_spawn_chance` (5.5), not the moon's brightness. The enderman's projectile
  refusal matters to section 1, and its block moving (`mob_griefing`, `nextInt(10)` to take,
  `nextInt(1000)` to leave) is visible within minutes of meeting one.
- **15.12 Natural spawning across regions.** (a) stands with two simplifications the bytecode
  allows. The **spawnable chunk count** is a function of the players' positions alone (the chunks
  within Chebyshev 8 of each tracked player's chunk, loaded or not, 5.2), so each region can
  compute it from the world's players' table; it need not be published. The **local cap** is per
  player, summed over the chunks whose centre lies within 128 blocks of that player (5.1), so the
  published per-chunk counts by category are exactly what is needed; a player for whom nothing has
  been counted passes. The inner question "does the local cap count the region's own changes at
  once" is yes (`afterSpawn` → `addMob`), and the near-players list of a chunk is fixed for the
  tick at its first use.
- **15.14 The nearest player.** Unchanged, but the local cap and the spawning chunks also take
  players from the world's table (all non-spectator players within 128 of the chunk), not only
  the nearest.
- **15.16 The custom spawners.** The reasons given are wrong for two of them.
  **Phantoms do not need survival**: `TIME_SINCE_REST` rises in every game mode while a player is
  awake (n7d ServerPlayer.txt 1091–1102) and the spawner skips only spectators (5.4). A creative
  player who has not slept for 72 000 ticks (an hour of play) gets one or two phantoms circling at
  night under open sky; they cannot target the player but are seen. That is not "within minutes",
  so naming it is still defensible, but the plan must name it for the right reason and keep a
  per-player time since rest if phantoms come later (Clustine's sleep resets it).
  **Patrols do not need raids**: the spawner needs a non-spectator player not within two sections
  of a village (points of interest; with none kept, nowhere is near a village), daylight, day 5 of
  the overworld clock by the `early_game` timeline, and `nextInt(5) == 0` every 12000 +
  `nextInt(1200)` ticks; `ceil(effectiveDifficulty) + 1` pillagers (2 at easy in a young world),
  the leader with an ominous banner (a component). A player sees one after about 100 minutes of
  play. Recommend: still none in N9, named with these reasons; patrols are the cheaper of the two
  once components and the pillager exist. The siege's daily `nextInt(10)` on the level random is
  drawn even without villages; Clustine need not reproduce a draw on a stream it keys differently.
- **15.19 Comparison with the official server.** Add, all without draws: the lingering cloud's
  radius over time and its application ticks; the fangs' event 4 and damage ticks for a fixed
  evoker; a splash's proximity for a target touching the potion's box (1.0); the crossbow's load
  tick while held. The arrow's refusal is testable with a mob summoned with `Invulnerable:1b`
  (motion × −0.1 and the yaw turn, one draw).
