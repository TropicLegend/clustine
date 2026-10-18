# N6 porting brief: TNT, explosions, containers and the redstone that waits for entities (26.3, protocol 777)

Read from `server-26.3.jar` with `javap -p -c -constants` (names are Mojang's), one file
per class, and from `target/datagen/26.3/generated/`. Every rule below is the bytecode's
unless marked **Inferred** (reasoning, to test) or **Client** (client code, not in the
server jar). Method names are the game's; "line" ranges, where given, are of the `javap`
output of that class ("ServerExplosion.txt" is
`net.minecraft.world.level.ServerExplosion`'s,
and so on), so a porter can go straight to them. The step's own dump is
`reference/n6-javap/` (158 classes); the N5 dump, `reference/n5-javap/`, has the rest
quoted here (`LivingEntity`, `Player`, `ServerPlayer`, `ServerLevel`, `Level`,
`AbstractMinecart`, `Containers`, `ItemStack`, …):

```
J=target/datagen/26.3/versions/26.3/server-26.3.jar
$HOME/.local/jdk/bin/javap -p -c -constants -cp $J net.minecraft.world.level.ServerExplosion
```

The classes read are those of the step: `PrimedTnt` (and `$1`), `TntBlock`,
`MinecartTNT`, `ServerExplosion` (and `$StackCollector`), `Explosion` (and
`$BlockInteraction`), `ExplosionDamageCalculator`, `EntityBasedExplosionDamageCalculator`,
`ClientboundExplodePacket`; `ChestBlock` (and its nested classes), `ChestBlockEntity`,
`ChestLidController`, `ContainerOpenersCounter`, `BarrelBlock`, `BarrelBlockEntity`,
`ShulkerBoxBlock`, `ShulkerBoxBlockEntity`, `BaseContainerBlockEntity`,
`RandomizableContainerBlockEntity`, `BlockEntity`, `BlockEntityType(s)`, `Container`,
`SimpleContainer`, `CompoundContainer`, `ContainerHelper`, `Containers`; the menus
(`AbstractContainerMenu`, `ChestMenu`, `DispenserMenu`, `HopperMenu`, `CrafterMenu`,
`ShulkerBoxMenu`, `Slot`, `SlotRange(s)`, `ContainerSynchronizer`, `MenuType`), the
container packets, `HashedStack`, `HashedPatchMap`, `ServerGamePacketListenerImpl`;
`DispenserBlock`, `DropperBlock`, their block entities, `BlockSource` and every class of
`net.minecraft.core.dispenser`; `CrafterBlock`, `CrafterBlockEntity`; `HopperBlock`,
`HopperBlockEntity`, `Hopper`, `MinecartHopper`; `BasePressurePlateBlock`,
`PressurePlateBlock`, `WeightedPressurePlateBlock`, `TripWireBlock`, `TripWireHookBlock`,
`RedStoneOreBlock`; `ComparatorBlock`; `SculkSensorBlock`, `CalibratedSculkSensorBlock`,
`SculkSensorBlockEntity` (and its `VibrationUser`), `VibrationSystem` (and `$Data`,
`$Listener`, `$Ticker`, `$User`); and `Entity` with its nested classes. What a rule still
depends on and nobody has read is in section 12. `invokedynamic` targets (lambdas and
method references) are not shown by `javap -c`; where a rule depends on which method one
is, it is marked **Inferred**. SteelMC's 26.3 extraction is quoted only where 26.3 keeps
a value in a class not dumped (game rules' defaults, level event names, fluids'
resistance), and says so where it is.

Conventions throughout (as in `docs/groundwork/n4-brief.md` and `n5-brief.md`): `double`
is `f64`, `float` is `f32`; a `float` constant used in `double` arithmetic is widened
(`f2d`) *after* any float arithmetic it takes part in; no fused multiply-add (never call
`mul_add`); `Mth.floor(double)` is `(int)Math.floor(x)`, which Rust's `x.floor() as i32`
matches; Java `int` arithmetic wraps and `%` (also `frem` on floats) takes the sign of
the dividend, as Rust's `%` does; `Mth.sin`/`Mth.cos` are the game's table (the repo's
`clustine_noise::trig`), and `Math.sin`/`Math.cos` (TNT's priming) are ported with
`libm` as ADR-0028 says.

Each of sections 1 to 8 opens with the findings of its own reading (x.0); section 0
gathers those that change what the plan, ADR-0025, ADR-0026 and ADR-0028 assume, with
pointers. Sections 9 to 12 gather, by subject, the packets, the random draws, the line
between regions and what was not read.

---------------------------------------------------------------------------------------

## 0. What the step should know before planning (findings)

**What N6 is larger than the plan's line says**

1. **N6 brings the first container menus.** The plan's line (n-plan section 4: "TNT and
   explosions, and the redstone of ADR-0026 section 11 that waits for entities") does
   not name them, but a creative player fills a dispenser, a dropper or a hopper only
   through its menu, and today a click on a chest does nothing (no menu is opened
   anywhere in `services/edge`). The menus are a protocol of their own: `open_screen`,
   the click with its seven kinds (`ClickType` is now `ContainerInput`, wire ids 0 to 6
   **Inferred**), hashed stacks, a state id, `container_set_content`/`_slot`/`_data`,
   `set_cursor_item`, closing (3.0 items 15–26; 3.10–3.21). Chests, barrels and shulker
   boxes come with them at little more cost, with their openers' count, lids, block
   events and sounds (3.1–3.5).
2. **The split between the edge and the region falls on the game's own seam,
   `ContainerSynchronizer`** (ADR-0005): the region runs `AbstractContainerMenu`'s rules
   on the container and inventory it holds; the edge keeps the container id (1 to 100),
   the state id (15 bits), the remote slots and their hashes, and sends every packet
   (3.0 items 17, 19; 3.21). A click whose state id is stale still runs, and the whole
   menu is sent again after it; a click on another container id, a menu no longer
   valid or a slot out of range is refused without an answer (3.0 item 16).
3. **Game events do not exist in Clustine**, and N4 and N5 skipped every post. Sculk
   sensors (section 8) need them posted from each place the game makes one: steps,
   landings, splashes, riding, N5's entities placed, item frames, levers, buttons,
   doors, pistons, note blocks, plates, tripwire, containers opening and closing, TNT
   primed, explosions, blocks broken and placed. The protocol crate's `GameEvent` is the
   unrelated "change game state" packet; the simulation's need another name (8.0 items
   1, 2). Players' steps and falls are heard only if the region runs them on the moves
   the edge reports, and **sneaking has to become region state** (`region/pressed.rs`
   takes no player as sneaking today) (8.0 items 3, 4). Sculk sensors could be left out
   of N6 and named; if they are kept, the posts are most of their work.

**What the data lacks**

4. **Block explosion resistance is missing** from Clustine's data and from
   `reports/blocks.json`; the datagen's `Extract.java` has to write
   `getExplosionResistance()` per block beside the physics it writes; fluids' are 100 for
   water and lava (SteelMC's 26.3 extraction) (2.0 item 10).
5. **Item stacks have no components** (`clustine_world::entity::ItemStack`). Menus work
   without them, but a shulker box keeps its contents in its item (`container`
   component, `copy_components` in its loot), so a shulker box broken by a creative
   player, by an explosion or dispensed would lose its contents; named and enchanted
   stacks lose their components through a menu, and a client's hash with components
   never matches, so the slot is resent at each click. **A decision for the plan**:
   components in stacks in N6, or shulker boxes named as a difference (3.0 item 18; 2.6;
   3.5).
6. **Container loot tables are missing**: structure chests, barrels, dispensers and
   hoppers keep `LootTable`/`LootTableSeed`, unpacked when first opened, read by a
   comparator, pulled from by a hopper, dispensed from or broken; Clustine generates only
   block loot tables (3.0 item 11; 5.0 item 7; 4.0 item 2).

**What 26.3 is**

7. **There is no `onCaughtFire`** (a Forge hook): fire primes TNT from `FireBlock`'s
   burning out, which Clustine's `region/random_ticks/fire.rs` already marks. Every
   priming draws one `nextDouble` of the level's random for the motion's angle (turned by
   `Math.sin`/`Math.cos`); a TNT primed by an explosion draws `nextInt(20)` too, for a
   fuse of 10 to 29 ticks. `PrimedTnt` has no `baseTick` (no void check, no burning);
   its fuse is synched data that changes every tick, so it is sent every tick; it
   explodes with radius 4 at `y + 0.06125` (1.0 items 1–5).
8. **TNT drops everything it breaks**: its interaction is `DESTROY` while the game rule
   `tnt_explosion_drop_decay` is false (its default, from SteelMC's extraction), so the
   loot has no radius and draws nothing for `survives_explosion` or `explosion_decay`; the
   loot has the TNT as `THIS_ENTITY`, so chorus flowers and snow layers drop; drops are
   merged across blocks into stacks of at most 16, each laid at its first block, after
   every block is broken (2.0 items 4, 5).
9. **Entities are hurt before any block breaks**; the damage is a `float`; creative
   players are not hurt but are knocked back unless flying, and that knockback reaches
   the client only in the explosion packet (`explode`, clientbound 36), which goes to
   every player of the level within 64 blocks of the centre, not to an entity's
   trackers: the edge's to fan out across regions (2.0 items 6, 8, 9).
10. **The owner decides what entities drop**: a creative player's TNT discards vehicles
    and breaks frames, paintings and cushions without their items; TNT lit by redstone
    or fire has no owner and they drop. Blocks drop either way (2.0 item 7).
11. **New game rule `tnt_explodes`** gates `PrimedTnt.explode`, `TntBlock.prime` and the
    dispenser's TNT; its default is not dumped (**Inferred** true) (4.0; section 12).
12. **Dispensers and droppers fire 4 ticks after a rising edge** of
    `hasNeighborSignal(pos) || hasNeighborSignal(pos.above())` and do not look at the
    power again; the slot is one level `nextInt(n)` per non-empty slot (reservoir
    sampling, `n = 1, 2, …`). 82 items are registered by name; equippable items go onto
    players and armour stands in front; **fire charges are projectiles in 26.3**, and the
    13 projectile items wait for N9 (dropped as items until then is recommended, named).
    A dropper puts one item into the container in front silently and keeps it when it
    does not fit. Several fallbacks play the click and smoke twice. Shears cut N5's
    leashes. The sulfur cube (a stable 26.3 mob) is N7's, but its tag is checked on every
    dispense of a block (4.0; 4.4–4.18).
13. **Hoppers move items in the entity phase as well as their own**: an item entity's
    block walk meeting a hopper runs the hopper's whole `tryMoveItems` (eject, then take
    that entity). Clustine's item walk (`physics/item.rs`) is pure and has to hand
    hoppers, plates, tripwire and detector rails back to the region, as `CartInside`
    does. The cooldown is 8, handed on to an empty hopper as 8 or 7 by whether it has
    ticked this game tick (`tickedGameTime`, not saved). A hopper's choice among
    container entities is a draw of the level's random. A hopper cart has no cooldown,
    never pushes, and keeps an `Enabled` flag (saved, not synched) that Clustine's
    minecart record lacks (5.0).
14. **Plates count every entity but spectators and marker armour stands** (stone plates
    living ones only, so in N6 only players and armour stands); a weighted plate counts
    entities, not items, `ceil(min(n, max)/max · 15)`; which plates are `MOBS` is in
    `BlockSetType`, not dumped (**Inferred** stone and polished blackstone). Tripwire is
    pressed by the entity that walked in and kept by any entity. A detector rail needs
    only that any entity entering it tells comparators (6.0).
15. **Containers**: every container block consumes the click, opening or not; a chest
    under a conductor opens nothing and reads 0 to a comparator; a double chest is one
    `CompoundContainer` with the `right` half first, two counts, two block events, one
    sound at the pair's centre; chests and barrels recheck their openers every 5 ticks by
    a scheduled block tick and send the block event each time; the shulker box's lid is
    simulated on the server, changes its shape and pushes entities; breaking any
    container but a shulker box spills its contents, in creative too; no container sends
    `block_entity_data` (3.0 items 1–14).
16. **The crafter needs the whole crafting recipe matcher** (1 266 recipes of 14 types
    in the generated data); leaving it to the step that brings crafting is recommended
    (7.0).

**What a step can and cannot do across regions: the explosion**

17. An explosion is computed whole inside one entity's tick (the TNT's, in the entity
    phase), at once, in the region that ticks the TNT, in this order
    (`ServerExplosion.explode`, lines 849–885). Every random draw is the level's unless
    said; section 10.1 numbers them.
    1. **Game event** `explode` at the centre (sculk sensors in range, which may be across
       the line).
    2. **Rays** (2.4): 1352 `nextFloat`, one per ray, in the fixed order `i, j, k`.
       **Reads** block and fluid states at up to 24 points per ray (for TNT the centre
       and 23 steps of 0.3: within 6.9 blocks, so in the TNT's chunk or a neighbour,
       within the 3×3 of border copies of ADR-0025 section 3). A ray's length depends on
       every block it passes, so a position across the line is recorded or not by the
       copy's state of the blocks on the way. **Writes** nothing.
    3. **Entities** (2.5): **reads** the entities whose box meets the centre ± `(2r + 1)`
       (± 9 for TNT, within one chunk: the region's own and, across the line, the
       copies' entities), their positions, eyes, boxes, kinds, the players' game mode and
       flight, and for each the blocks between its sample points and the centre (`clip`,
       collision shapes, no fluids), up to 9 blocks away, as they were before this
       explosion. **Writes**, per entity in the query's order: its damage (an item's
       health, a vehicle's damage and breaking, an armour stand's death, a frame's or
       painting's breaking, a TNT minecart's priming with two `nextInt(20)` of the
       cart's own random), its motion (`push`), and a player's knockback for the packet.
    4. **Shuffle** (2.6): `n − 1` draws, `nextInt(n)` down to `nextInt(2)`, over the
       recorded positions, air included.
    5. **Blocks**, one at a time in the shuffled order: **reads** the block and its block
       entity; its loot (the table's own draws, none of the explosion's for TNT);
       **writes** air with flag 3 and runs that change's updates at once (falling blocks,
       fluids, attached blocks popping off); a container **spills** its contents; a TNT
       **primes** (one `nextDouble`, one `nextInt(20)`).
    6. **Drops**, per merged stack, laid by `popResource` at its first block (three
       `nextDouble`, then the item's own draws, N4 brief 8.4), possibly across the line
       from blocks whose drops it holds.
    7. **Fire** (not TNT's): per position one `nextInt(3)`, then a set.
    8. **The packet** to every player within 64 blocks, with that player's knockback.
18. **A step can** compute all of it in the TNT's region: everything it reads lies
    within one chunk of the TNT, which the copies cover, and every draw is that region's
    (the game's level random is not reproduced in any case; the oracle judges by
    distributions, n-plan section 5).
19. **A step cannot** make the far side's changes in the same tick as the near side's: a
    block across the line becomes a conditional set (`SetIf`: air if still the block the
    copy showed, ADR-0028 section 7), carried as an effect, a tick late while coupled;
    an entity across it is hit by an entity effect (damage source, amount, push). So the
    far side's updates, spills, chained TNT and drops come a tick later and in the far
    region's order; a ray that passed a block that changed in the copy's last tick ends
    otherwise than in the game (rare).
20. **The far side must own the loot of its blocks**: their block entities (a
    container's contents, what a table copies from the block entity) are only in the
    holder, and the copy has the blocks alone. So the block effect has to carry the
    call, not only the expected state: "broken by an explosion of this radius and
    interaction, from this entity, with this owner", and the holder evaluates the loot,
    spills the container, primes TNT and lays the drops, which **cannot be merged with
    the near side's** (a named difference: more, smaller stacks at a line). ADR-0028
    section 7 lists "an explosion's damage and knockback, with its blocks as block
    effects (`SetIf`)"; this is what it has to add. Where regions follow players, an
    explosion at a line is a reason to merge, as entities working there are.

**The other things that meet the line** (section 11 has each in full)

21. **A menu needs its container in the player's own region**: opening, every click and
    `stillValid` read and write the container and the inventory together, and ADR-0026
    section 7 already refuses a click on land not held. Proposed: a container opens only
    where the player's region holds all its chunks (both halves of a double chest);
    regions that follow players merge; at a pinned line it is a named difference. An
    opener in another region is not counted by the 5-tick recheck, so a lid would close
    after 5 ticks (3.0 item 6; 11.2, 11.3).
22. **A dispenser, dropper or hopper facing across a line** reads and writes another
    region's block or container: placing a block or entity, inserting an item. Options
    (11.4, 11.5, **Inferred**): the piston's rule (it waits, ADR-0026 section 10), or an
    offer to the holder that inserts or answers with what is left. Pulling never crosses
    by blocks (the hopper's source is above it, in its own column), only by entities.
23. **Plates, tripwire and sculk sensors at a line** see only the region's own entities
    today; an entity across the line on a plate needs an effect to the plate's holder,
    and a vibration within 8 blocks of a sensor across the line has to go as an effect
    carrying the event, its tick, the source's flags (spectator, sneaking, wool) and the
    affected block; the occlusion reads up to 8 blocks into the far side, within the
    copies. A sensor's "3×3 chunks ticking" test has to read "its chunk ticks" at a line,
    or a sensor next to a line never fires (8.0; 11.5, 11.6).

---------------------------------------------------------------------------------------

## 1. TNT

The block (`TntBlock.txt`, 356 lines), the entity (`PrimedTnt.txt`, 459 lines, and
`PrimedTnt$1.txt`, the calculator of TNT that has been through a portal), and the TNT
minecart in brief (`MinecartTNT.txt`, 526 lines; N5 brief section 3.19 has it whole).
Line numbers in this section are of those files unless another is named.

### 1.0 Findings of this section

1. **There is no `onCaughtFire` in 26.3.** `TntBlock` has `onPlace`, `neighborChanged`,
   `playerWillDestroy`, `wasExploded`, the two `prime`s, `useItemOn`, `onProjectileHit`
   and `dropFromExplosion` (false), and nothing else (lines 21–356). `onCaughtFire` is a
   Forge hook, not the game's. Fire primes TNT from `FireBlock`'s burning out, which
   calls `TntBlock.prime(level, pos)` itself after it has replaced the block (FireBlock
   is not dumped; Clustine's port says where: `region/random_ticks/fire.rs`, "Burnt TNT
   would be primed here"). The ways TNT is primed are, then: redstone (placed powered,
   or a neighbour change while powered), fire burning it, a flint and steel or a fire
   charge used on it, a burning projectile (N9), an explosion (`wasExploded`, with a
   short random fuse), a dispenser's flint and steel facing it and a dispensed TNT item
   (section 4), and a survival player breaking *unstable* TNT. A creative player's break
   never primes it, unstable or not (the `instabuild` test, lines 61–86).
2. **Every priming draws one `nextDouble` from the level's random** for the entity's
   initial motion (constructor, lines 43–89), and the angle is turned by `Math.sin` and
   `Math.cos` of a `double` (not the game's table): `(−sin(a)·0.02, 0.2f widened,
   −cos(a)·0.02)` with `a = nextDouble()·6.2831854820251465`. ADR-0028 already names
   this as one of HotSpot's three `Math.sin`/`Math.cos` uses ported with `StrictMath`
   (`libm`, as `clustine-noise` does). A TNT primed by an explosion draws a second time,
   `nextInt(20)`, for its fuse of 10 to 29 ticks (lines 88–129, 367–380).
3. **PrimedTnt's tick has no `baseTick`** (lines 126–210): it does not burn, is not
   hurt by lava or the void (no `checkBelowWorld`: TNT falling out of the world still
   explodes when its fuse runs out), and its fire, freezing and air are never counted.
   It is: portal, gravity 0.04, `move`, the blocks' effects, drag `0.98f`, on the ground
   `× (0.7, −0.5, 0.7)`, fuse down by one, and at 0 `discard()` then explode; otherwise
   the fluids' push (`updateFluidInteraction`). It cannot be hurt (`hurtServer` is
   false, lines 425–428) but is pushed by other explosions, and its push is taken from
   its feet (`position()`), not its eyes: the TNT cannon's rule (2.5).
4. **The fuse is synched data and changes every tick**, so `ServerEntity` sends the
   entity every tick (the dirty data opens `sendChanges`' gate, N5 brief 1.11.1), with
   `setRequiresPrecisePosition` set on any collision, which makes that tick's move a
   whole `entity_position_sync` (lines 147–209). Its type's update interval (10) and
   tracking range (10 chunks) are thus moot while it burns.
5. **A primed TNT's explosion is radius 4.0 at `y + 0.98·0.0625`** (`getY(0.0625)`,
   the box height `0.98f` widened times 0.0625, EntityTypes.txt 2790–2806), not at its
   centre, with the TNT itself as the source, the owner as the damage's cause, no fire
   and the `TNT` interaction, whose block interaction is `DESTROY` by default
   (`tnt_explosion_drop_decay` is false): **every block a TNT destroys drops its loot
   whole** (section 2.0 item 4).
6. **`usedPortal` is not saved** (lines 257–319 write `fuse`, `block_state`,
   `explosion_power` when not 4, `owner`): a TNT that went through a portal and is
   reloaded forgets it and breaks portal blocks again. Portals are not in Clustine yet;
   the calculator (`PrimedTnt$1`) is named for when they are.
7. **The owner is kept and passes on**: the igniting player (flint and steel, fire
   charge) or the shooter (a burning arrow), none for redstone and fire; a TNT primed by
   an explosion gets that explosion's `getIndirectSourceEntity()` (lines 88–129), so a
   chain keeps the first igniter. The owner decides what the explosion's damage source
   is and whether vehicles, frames, paintings and cushions it breaks drop their item
   (2.0 item 7).
8. **The TNT minecart is N5's, inert; its explosion is N6's.** Its power is
   `4 + 1.0·nextDouble()·1.5·min(√speed², 5)` (the cart's own random; up to 11.5), from
   its own position, and while primed it neither breaks nor is stopped by rails and the
   block under a rail (1.6).

### 1.1 The block

The block has one property, `unstable` (false by default, constructor lines 5–19,
`createBlockStateDefinition`). Its loot table (`loot_table/blocks/tnt.json`) is one pool
under `survives_explosion` with the TNT item when `unstable` is false; it never drops
from an explosion anyway (`dropFromExplosion` false, lines 353–356) but is primed
(`wasExploded`, 1.2).

**`onPlace(state, level, pos, oldState, movedByPiston)`** (lines 21–42):

```
if oldState.is(this.block): return            // a change of unstable only
if level.hasNeighborSignal(pos) && prime(level, pos):
    level.removeBlock(pos, false)
```

**`neighborChanged(state, level, pos, block, orientation, movedByPiston)`** (lines
44–59): the same two lines without the first test. Both prime with no owner. A TNT
pushed by a piston onto a powered place is primed on arrival (`onPlace` from the moving
piston's placement; the old state is the moving piston, not TNT).

`hasNeighborSignal` is the redstone's (ADR-0026, `redstone.rs`): any of the six
neighbours gives a signal into this block. A signal across a region line is read from
the copy (ADR-0026 section 10).

**`playerWillDestroy(level, pos, state, player)`** (lines 61–86): on the server, if the
player is not `instabuild` and `unstable` is true, `prime(level, pos)` (no owner); then
`Block.playerWillDestroy`. Creative players never reach the prime.

**`prime(level, pos)`** (lines 131–138) = `prime(level, pos, null, ItemStack.EMPTY)`.

**`prime(level, pos, owner, stack)`** (lines 140–220):

```
if !(level is ServerLevel) || !gamerule tnt_explodes: return false
if owner is Player && owner.gameMode() == ADVENTURE
        && !stack.canBreakBlockInAdventureMode(BlockInWorld(level, pos, false)):
    return false
tnt = new PrimedTnt(level, pos.x + 0.5, pos.y, pos.z + 0.5, owner)    // the draw, 1.3
level.addFreshEntity(tnt)
level.playSound(null, tnt.x, tnt.y, tnt.z, TNT_PRIMED, BLOCKS, 1.0f, 1.0f)
level.gameEvent(owner, PRIME_FUSE, pos)
return true
```

The block itself is not touched here; every caller removes it after a true answer.
`x + 0.5` and `z + 0.5` are `double` (`i2d`, then `dadd 0.5`); `y` is the block's floor.
The sound is `entity.tnt.primed` (1627) in the block source, at the entity, to everyone
in range (the `null` player), its seed from the level's sound seed generator
(Level.txt 746–761, n5 dump), not the level's random. `prime_fuse` is game event 35
(section 8).

**`useItemOn(stack, state, level, pos, player, hand, hit)`** (lines 222–299):

```
if !stack.is(FLINT_AND_STEEL) && !stack.is(FIRE_CHARGE):
    return Block.useItemOn(...)                         // TRY_WITH_EMPTY_HAND: nothing
if prime(level, pos, player, stack):
    level.setBlock(pos, AIR, 11)                        // UPDATE_NEIGHBORS | UPDATE_CLIENTS | UPDATE_IMMEDIATE
    item = stack.getItem()
    if stack.is(FLINT_AND_STEEL): stack.hurtAndBreak(1, player, hand.asEquipmentSlot())
    else: stack.consume(1, player)
    player.awardStat(ITEM_USED.get(item))
else if level is ServerLevel && !gamerule tnt_explodes:
    player.sendOverlayMessage(translatable("block.minecraft.tnt.disabled"))
    return PASS
return SUCCESS
```

For a creative player neither the flint and steel's durability nor the fire charge's
count changes (**Inferred**: `hurtAndBreak` and `consume` skip a player with infinite
materials; `ItemStack` of the n5 dump has them, not reread here). The set is flag 11,
not `removeBlock`'s 3: the neighbours are updated and the client sent the change, and the
set is immediate. The order is: entity, sound, game event, then the block goes. Whether
the block's `useItemOn` is reached at all with a flint and steel follows
`ServerPlayerGameMode.useItemOn` (N5 brief 2.9): a sneaking player holding an item skips
the block and uses the item, so a flint and steel then sets fire beside the TNT (which
primes it only when the fire burns it).

**`onProjectileHit(level, state, hit, projectile)`** (lines 301–351), N9: on the server,
if the projectile is on fire and `mayInteract(level, pos)`, `prime(level, pos, owner if
it is a LivingEntity, the arrow's pick-up stack or EMPTY)` and on success
`removeBlock(pos, false)`.

### 1.2 `wasExploded(level, pos, explosion)` (lines 88–129)

Called by the explosion after it has set the block to air (2.6):

```
if !gamerule tnt_explodes: return
tnt = new PrimedTnt(level, pos.x + 0.5, pos.y, pos.z + 0.5,
explosion.getIndirectSourceEntity())
tnt.setFuse(PrimedTnt.getRandomShortFuse(tnt.getFuse(), level.getRandom()))
level.addFreshEntity(tnt)
```

`getRandomShortFuse(fuse, random)` (lines 367–380) = `random.nextInt(max(1, fuse / 4)) +
fuse / 8` in `int`, so with the constructor's 80: `nextInt(20) + 10`, 10 to 29 ticks. No
sound and no game event (unlike `prime`). Two draws from the level's random in order:
the constructor's `nextDouble`, then this `nextInt(20)`.

### 1.3 `PrimedTnt` construction

`PrimedTnt(type, level)` (lines 29–41): `Entity.<init>` (its own random's draws: the
UUID, two `nextLong`, N4 brief 8.4), `explosionPower = 4.0f`, `blocksBuilding = true` (a
block cannot be placed into it).

`PrimedTnt(level, x, y, z, owner)` (lines 43–89), in order:

```
this(EntityTypes.TNT, level)
setPos(x, y, z)
a = level.getRandom().nextDouble() * 6.2831854820251465     // the level's random
setDeltaMovement(-Math.sin(a) * 0.02, 0.20000000298023224, -Math.cos(a) * 0.02)
setFuse(80)
xo, yo, zo = x, y, z
owner = EntityReference.of(owner)                             // null stays null
```

`6.2831854820251465` is `(float)(2π)` widened, and `0.20000000298023224` is `0.2f`
widened. `Math.sin` and `Math.cos` of a `double` (lines 62, 68): HotSpot's intrinsics,
ported with `libm` (`StrictMath`) as ADR-0028 section 4 says; a value within an ulp
either way. The motion is a vector of length 0.02 horizontally in a uniformly drawn
direction, and 0.2 up.

The type (EntityTypes.txt 2790–2806): `noLootTable`, `fireImmune`, `sized(0.98f,
0.98f)`, `eyeHeight(0.15f)`, `clientTrackingRange(10)`, `updateInterval(10)`, category
`MISC`. Protocol id 136 (`registries.json`).

Other overrides: `getMovementEmission()` is `NONE` (lines 106–109: no step sounds and no
step or swim game events); `isPickable()` is `!isRemoved()` (lines 111–119: the client
can target it, a player's attack reaches `hurtServer`, which refuses);
`getDefaultGravity()` is `0.04` (lines 121–124); `getMoveSimulationType()` is
`SERVER_AND_CLIENT` (lines 430–433; **Client**: the client runs the same tick between
updates).

### 1.4 `tick()` (lines 126–210), in order

```
handlePortal()                                              // portals: later
applyGravity()                                              // motion.y -= 0.04
(Entity.txt 3924–3942; 0 with NoGravity)
move(SELF, getDeltaMovement())                              // N4's Entity.move
applyEffectsFromBlocks()                                    // N4's: cobweb, honey, fire, …
setDeltaMovement(getDeltaMovement().scale((double) getAirDrag()))   // 0.98f widened:
0.9800000190734863
if onGround(): setDeltaMovement(getDeltaMovement().multiply(0.7, -0.5, 0.7))
fuse = getFuse() - 1
setFuse(fuse)
if fuse <= 0:
    discard()
    if server: explode()
else:
    updateFluidInteraction()                                // the fluids' push; result unused
    (client: a smoke particle at (x, y + 0.5, z))
setRequiresPrecisePosition(horizontalCollision || verticalCollision)
```

- The drag is applied before the ground's bounce: on the ground the motion is `× 0.98`
  then `× (0.7, −0.5, 0.7)`. `0.7` and `-0.5` are `double` constants.
- `applyEffectsFromBlocks` makes fire set the entity on fire (it is `fireImmune`, so
  nothing comes of it) and lets a TNT press a pressure plate it rests on (section 6) and
  trip a tripwire.
- No `baseTick`: no `checkBelowWorld`, no lava, no burning count, no air, no freezing.
  A TNT in the void explodes there.
- The fluids are not updated on the tick the fuse runs out, nor before the first move;
  a TNT in water is pushed by the current from the tick after the one in which it was
  first in it (**Inferred** from the order; `updateFluidInteraction` is N4's).
- The fuse at priming is 80: the TNT explodes on its 80th tick (4 s), after that tick's
  move. A fuse of 1 at the start of a tick explodes in that tick; a fuse loaded as 0 or
  less explodes on its first tick.

### 1.5 `explode()` (lines 212–255)

```
if level is ServerLevel && gamerule tnt_explodes:
    level.explode(this,
        Explosion.getDefaultDamageSource(level, this),     // explosion(this, owner) (2.1)
        usedPortal ? USED_PORTAL_DAMAGE_CALCULATOR : null,  // null: an EntityBasedExplosionDamageCalculator(this)
        getX(), getY(0.0625), getZ(),                       // y + (double)0.98f * 0.0625
        explosionPower,                                     // 4.0f unless saved otherwise
        false,                                              // no fire
        Level.ExplosionInteraction.TNT)
```

`getY(0.0625)` is `position.y + (double)getBbHeight() * 0.0625` (Entity.txt
10842–10853): `y + 0.06125000074505806`. The entity was discarded just before, and the
explosion's entity query leaves out its source anyway. With `tnt_explodes` off (a TNT
from before the rule changed, or summoned) it is discarded without a sound.

`USED_PORTAL_DAMAGE_CALCULATOR` (`PrimedTnt$1`, lines 9–41): `nether_portal` blocks give
no resistance (an empty `Optional`, as air) and are not destroyed
(`shouldBlockExplode` false); everything else as the plain calculator. It is set by
`teleport(transition)` (lines 407–423) on the TNT that arrives through a portal, and not
saved.

### 1.6 The TNT minecart (MinecartTNT.txt; N5 brief 3.19)

N5 has the whole class; what N6 adds is its explosion and its priming by things N6 brings.

- **Priming**: a powered activator rail (`activateMinecart`, lines 270–280: powered
  and `fuse < 0` → `primeFuse(null)`); destruction by a burning projectile, a fire or
  explosion source (`destroy`, lines 133–169: `primeFuse(source)` then `fuse =
  random.nextInt(20) + random.nextInt(20)`, the cart's own random, so 0 to 38 ticks);
  any destruction while it moves at `horizontalDistanceSqr ≥ 0.009999999776482582`
  (`0.01f` widened) primes it the same way.
- `primeFuse(source)` (lines 296–357): nothing with `tnt_explodes` off; `fuse = 80`; on
  the server an ignition source `explosion(source.getEntity())` if there was a source and
  none yet; entity event 70 (the client's own fuse and flashing); unless silent,
  `entity.tnt.primed` (blocks, 1, 1) at the cart.
- **Exploding** at once (not primed): a burning arrow hitting it (`hurtServer`, lines
  99–131, with the arrow's `lengthSqr` as the speed), a fall of 3 or more
  (`causeFallDamage`, lines 246–268, speed² `(fall/10)²`), a horizontal collision at
  `horizontalDistanceSqr ≥ 0.01f` widened (`tick`, lines 42–97), and the fuse reaching 0.
- `explode(source, speedSqr)` (lines 184–244): with `tnt_explodes`, `level.explode(this,
  source, null, x, y, z, (float)(explosionPowerBase + explosionSpeedFactor ·
  random.nextDouble() · 1.5 · min(√speedSqr, 5.0)), false, TNT)` and `discard()` (the
  arithmetic in `double`: `explosionPowerBase` and `explosionSpeedFactor` widened, then
  `d2f`; one `nextDouble` of the cart's own random); without the rule `discard()` only
  if primed. The explosion is at the cart's position, without an offset.
- While **primed** (`fuse > −1`), `getBlockExplosionResistance` is `0.0f` and
  `shouldBlockExplode` false for a block in `#minecraft:rails` or one under such a
  block (lines 376–429): the rays pass through rails and their beds unweakened and do
  not break them. Unprimed (an explosion caused by speed alone, not through
  `primeFuse`), they are ordinary blocks. The calculator is the
  `EntityBasedExplosionDamageCalculator` of the cart (2.2), which asks these.
- The cart's own state needs no synched data; the client learns of priming from event
  70 (N5 brief 3.19).

### 1.7 Synched data and what is saved

| Index | Accessor | Serializer | Default |
|---|---|---|---|
| 8 | `DATA_FUSE_ID` | `INT` (1, a VarInt) | `80` |
| 9 | `DATA_BLOCK_STATE_ID` | `BLOCK_STATE` (14, a VarInt of the state id) | `tnt` default (`unstable=false`) |

(`defineSynchedData` lines 91–104, static lines 441–458; Entity's own are 0 to 7.) The
block state is what the client draws; summoned TNT can carry another (`block_state`).

Saved (`addAdditionalSaveData`, lines 257–286): `fuse` as a **short** (`i2s` of the
int), `block_state` (`BlockState.CODEC`), `explosion_power` (a float) only when it is
not `4.0f`, `owner` (`EntityReference.store`: the UUID). Read (lines 288–319): `fuse`
by `getShortOr(…, 80)`, `block_state` or the default, `explosion_power` by
`getFloatOr(…, 4.0f)` clamped to `[0, 128]` (`Mth.clamp`, float), `owner`. `usedPortal`
is neither saved nor read. `restoreFrom` (lines 330–345) copies the owner across a
dimension change.

---------------------------------------------------------------------------------------

## 2. The explosion

`ServerExplosion.txt` (1064 lines), its `StackCollector` (34), `Explosion.txt` (77) and
`Explosion$BlockInteraction.txt` (102), `ExplosionDamageCalculator.txt` (85),
`EntityBasedExplosionDamageCalculator.txt` (59), `ClientboundExplodePacket.txt` (163);
and from the n5 dump `ServerLevel.explode` (ServerLevel.txt 2566–2700), `Level.explode`
(Level.txt 981–1063, the default particles 2162–2178), `Util.shuffle` (Util.txt
1957–1987), `Vec3.normalize` (Vec3.txt 173–), `EntityGetter.getEntities` (19–26),
`Level.isInWorldBounds` (161–173). Lines are of `ServerExplosion.txt` unless another
file is named.

### 2.0 Findings of this section

1. **The explosion's rays are drawn from the level's random, one `nextFloat` per ray,
   1352 rays, in a fixed order** (`calculateExplodedPositions`, lines 268–488): the
   surface of the 16³ grid, `i` (x) outermost, then `j` (y), then `k` (z), each 0 to 15,
   keeping those with any coordinate 0 or 15 (16³ − 14³ = 1352). The draw is made for
   every ray, including one whose first point is outside the world. A ray of strength
   `radius·(0.7 + 0.6·nextFloat)` (`float`) walks from the centre in steps of `0.3f`
   widened, losing `(resistance + 0.3)·0.3` at each point (also `float`) and `0.225`
   per step, and records the position while it is still positive. **The same block is
   read again and charged again at every point of a ray that falls in it** (two or three
   points per block): there is no memory along a ray.
2. **A TNT's rays reach 6.9 blocks**: the strength is below `4·1.3 = 5.2`, the loss at
   least `0.22500001f` a step, so at most 23 steps of 0.3 after the centre point. In
   general a ray reaches at most `0.3·⌈1.3r/0.225⌉`, about `1.73·r`. So a TNT's blocks
   lie in its own chunk or the next one in any direction: **always within the 3×3 of
   copies** a region keeps along its line (ADR-0025 section 3). Its entity box is the
   centre ± 9 (`2r + 1`, floored outwards), also within one chunk.
3. **The positions are collected in a `HashSet` and then shuffled with the level's
   random** (`interactWithBlocks`, lines 753–804: `Util.shuffle(list, level.random)`,
   which draws `nextInt(n)`, `nextInt(n−1)`, … `nextInt(2)`, `n − 1` draws). The set's
   order (by `BlockPos.hashCode`) is the shuffle's input, so the result is a uniformly
   random order whatever the input: **Clustine may collect in any fixed order** (sorted,
   say) and shuffle with its own stream; the distribution is the game's. The set
   **includes positions of air** (air and an empty fluid give no resistance, an empty
   `Optional`, and are recorded while the ray is positive, lines 413–454): they take
   part in the shuffle, count in the packet's `blockCount`, and are skipped when broken.
4. **TNT drops everything it breaks**: the `TNT` interaction is `DESTROY` unless the
   game rule `tnt_explosion_drop_decay` is true (default false); only
   `DESTROY_WITH_DECAY` gives the loot `EXPLOSION_RADIUS`, and without it
   `survives_explosion` holds without a draw and `explosion_decay` keeps every item
   (N4 brief 8.3; Clustine's `clustine_blocks::loot` already answers so). So TNT's drops
   are a rule's drops of the same blocks, with two differences: the loot has
   `THIS_ENTITY` (the TNT), so `entity_properties {"entity": "this"}` holds and **a chorus
   flower and a snow layer drop** (they drop nothing when broken by a rule), and the
   stacks are **merged across blocks** before they are laid (item 5). Block explosions
   (beds, respawn anchors) and mobs' decay by default (`block_explosion_drop_decay`,
   `mob_explosion_drop_decay` true), with `1/radius` per item. (**Inferred**:
   `BlockBehaviour.onExplosionHit`, which builds the loot parameters, is not dumped;
   the rule is the 1.21 one, 2.6.)
5. **Drops are merged into stacks of at most 16 and laid at the first block of each
   stack**, after all the blocks are broken (`addOrAppendStack`, lines 887–915;
   `StackCollector.tryMerge`, `ItemEntity.areMergable` and `ItemEntity.merge(a, b, 16)`).
   A stack merges into an earlier one when both are the same item with the same
   components and their sum fits the item's max stack; what is left over goes on to the
   next collector, and what is left at the end starts a new collector at the block it
   came from. The item of a merged stack thus appears at another block's position, which
   may lie across a region line from the block it came from.
6. **Entities are hurt before any block is broken** (`explode`, lines 849–885: game
   event, positions, `hurtEntities`, blocks, fire). The drops and the chained TNT of this
   explosion are not hit by it; they are hit by a later explosion of the same tick, which
   sees them (each `addFreshEntity` is at once). Exposure (`getSeenPercent`) is
   computed on the blocks as they were before this explosion.
7. **What the explosion does to an entity depends on the igniter**: the damage source is
   `explosion(tnt, owner)` (**Inferred**: `player_explosion` when there is an owner,
   `explosion` otherwise; `DamageSources` is not dumped); a vehicle hurt by the
   explosion of a creative player's TNT is discarded without its item, and a frame,
   painting or cushion broken by it drops nothing, since their `hurtServer` reads the
   source's entity, the owner (N5 brief 3.14, section 6, 5.8). TNT lit by redstone or
   fire has
   no owner: the vehicle breaks with its item when its damage passes 40, and hanging
   things drop. Blocks drop either way (item 4).
8. **Creative players are not hurt but are knocked back** unless they fly: every
   player not a spectator and not flying in creative is put in `hitPlayers` with its
   push, which goes to that player in the explosion packet; the client applies it
   (**Client**). The server also adds it to the player's own motion (`push`), which
   Clustine does not simulate (the client moves the player).
9. **The packet goes to every player of the level within 64 blocks of the centre**
   (ServerLevel.txt 2640–2686: `distanceToSqr(center) < 4096`), not to the trackers of
   an entity or a chunk, each with its own knockback or none. In Clustine that is the
   edge's to fan out to players of any region (section 9).
10. **Block resistance is not in Clustine's data.** `Block.getExplosionResistance()` (a
    property of the block from `Blocks`' `strength`) and `FluidState.
    getExplosionResistance()` (100 for water and lava, 0 for empty: SteelMC's 26.3
    `fluids.json`, for orientation) are needed for every block; `reports/blocks.json`
    has none. The datagen's `Extract.java` can write `getExplosionResistance()` per block
    beside the physics it already writes (section 12).

### 2.1 How an explosion starts: `ServerLevel.explode`

`Level.explode(source, damageSource, calculator, x, y, z, radius, fire, interaction)`
(Level.txt 1045–1062) adds the defaults: small particle `explosion` (30), large particle
`explosion_emitter` (29), block particles `DEFAULT_EXPLOSION_BLOCK_PARTICLES`, sound
`entity.generic.explode` (703). The default block particles (Level.txt 2162–2178): a
`WeightedList` of `ExplosionParticleInfo(poof, 0.5f, 1.0f)` and
`ExplosionParticleInfo(smoke, 1.0f, 1.0f)`, each added with the builder's default
weight (**Inferred**: 1; `WeightedList` is not dumped). `ExplosionParticleInfo`'s two
floats are **Inferred** to be a scaling and a speed (the class is not dumped).

`ServerLevel.explode(…, smallParticle, largeParticle, blockParticles, sound)`
(ServerLevel.txt 2566–2686):

```
blockInteraction = switch interaction:
    NONE    -> KEEP
    BLOCK   -> getDestroyType(block_explosion_drop_decay)
    MOB     -> mob_griefing ? getDestroyType(mob_explosion_drop_decay) : KEEP
    TNT     -> getDestroyType(tnt_explosion_drop_decay)
    TRIGGER -> TRIGGER_BLOCK
center = Vec3(x, y, z)
e = new ServerExplosion(this, source, damageSource, calculator, center, radius, fire,
blockInteraction)
count = e.explode()
particle = e.isSmall() ? smallParticle : largeParticle
for player in this.players:                                  // the level's list, in its
order
    if player.distanceToSqr(center) < 4096.0:
        knock = Optional.ofNullable(e.getHitPlayers().get(player))
        player.connection.send(ClientboundExplodePacket(center, radius, count, knock,
            particle, sound, blockParticles, source == null || !source.isSilent()))
```

`getDestroyType(rule)` (2688–2700): the rule true → `DESTROY_WITH_DECAY`, false →
`DESTROY`. The case labels' order is **Inferred** from the switch map (the enum
`Level$ExplosionInteraction` and `ServerLevel$1` are not dumped), and agrees with the
names of the rules each case reads.

Game rule defaults (SteelMC's 26.3 `game_rules.json`, orientation only; the game's
`GameRules` is not dumped): `tnt_explodes` true, `tnt_explosion_drop_decay` **false**,
`block_explosion_drop_decay` true, `mob_explosion_drop_decay` true, `mob_griefing` true.
All five are in the 26.3 registry `minecraft:game_rule` (`reports/registries.json`).

`Explosion$BlockInteraction` (lines 69–101): `KEEP` (ordinal 0,
`shouldAffectBlocklikeEntities` false), `DESTROY` (1, true), `DESTROY_WITH_DECAY` (2,
true), `TRIGGER_BLOCK` (3, false).

**The constructor** (lines 27–73): `hitPlayers = new HashMap()`; the fields; the damage
source is the given one or `damageSources().explosion(this)` when null; the calculator
is the given one or `makeDamageCalculator(source)` (lines 75–85): the plain
`EXPLOSION_DAMAGE_CALCULATOR` when there is no source entity, else
`new EntityBasedExplosionDamageCalculator(source)`.

`Explosion.getDefaultDamageSource(level, entity)` (Explosion.txt 3–11):
`damageSources().explosion(entity, getIndirectSourceEntity(entity))`.
`getIndirectSourceEntity(entity)` (13–60): a `PrimedTnt` → its owner; a `LivingEntity`
→ itself; a `Projectile` whose owner is a `LivingEntity` → that owner; anything else
(and null) → null. The explosion's own `getIndirectSourceEntity()` (lines 940–945) is
this of its source, `getDirectSourceEntity()` (947–951) the source.

Both damage types (`data/minecraft/damage_type/explosion.json`, `player_explosion.json`)
have `scaling: always` and exhaustion 0.1; both are in `#is_explosion`.

**Flags of the explosion**:
- `interactsWithBlocks()` (lines 917–926): not `KEEP`.
- `isSmall()` (1034–1047): `radius < 2.0f || !interactsWithBlocks()`. TNT (4, `DESTROY`)
  is large: `explosion_emitter`.
- `canTriggerBlocks()` (965–990): `TRIGGER_BLOCK` only (and for a breeze's wind charge
  only with `mob_griefing`). Never for TNT.
- `shouldAffectBlocklikeEntities()` (992–1032): `notWind = source == null || source is
  neither a breeze wind charge nor a wind charge`; with `mob_griefing` on, `notWind`;
  off, `blockInteraction.shouldAffectBlocklikeEntities() && notWind`. For TNT: true.
  This decides whether item frames, paintings, armour stands and the like are hit
  (their `ignoreExplosion`, 2.5).

### 2.2 The calculators

**`ExplosionDamageCalculator`** (ExplosionDamageCalculator.txt):

- `getBlockExplosionResistance(explosion, level, pos, state, fluid)` (lines 9–27):
  `state.isAir() && fluid.isEmpty()` → `Optional.empty()`; else
  `Optional.of(max(state.getBlock().getExplosionResistance(),
  fluid.getExplosionResistance()))`
  (`Math.max(FF)`). A waterlogged block resists as at least water (100): rays stop in
  it. Air holding a fluid (water, lava) resists 100.
- `shouldBlockExplode(…, power)` (29–32): true.
- `shouldDamageEntity(explosion, entity)` (34–37): true.
- `getKnockbackMultiplier(entity)` (39–42): `1.0f`.
- `getEntityDamageAmount(explosion, entity, seen)` (44–84):
  ```
  f = explosion.radius() * 2.0f                                   // float
  d = Math.sqrt(entity.distanceToSqr(explosion.center())) / (double) f
  impact = (1.0 - d) * (double) seen
  return (float) ((impact * impact + impact) / 2.0 * 7.0 * (double) f + 1.0)
  ```
  `distanceToSqr(Vec3)` is from the entity's position (its feet). The result is a
  `float`, not truncated: a TNT next to an entity, fully seen, deals up to `57.0`.

**`EntityBasedExplosionDamageCalculator(source)`** (EntityBased….txt):
`getBlockExplosionResistance` is the plain one mapped through
`source.getBlockExplosionResistance(explosion, level, pos, state, fluid, r)` (lines
14–31, 45–58; `Entity`'s returns `r`, Entity.txt 9330–9333); `shouldBlockExplode` is
`source.shouldBlockExplode(…)` (33–43; `Entity`'s true, 9335–9338). The others are
inherited. Only `MinecartTNT` overrides the two among the entities met so far (1.6).
For a primed TNT the result is the plain calculator's.

### 2.3 `explode()` (lines 849–885), in order

```
level.gameEvent(source, GameEvent.EXPLODE, center)     // section 8: game event 23
positions = calculateExplodedPositions()               // 2.4: 1352 level-random floats
hurtEntities()                                         // 2.5
if interactsWithBlocks(): interactWithBlocks(positions) // 2.6: the shuffle, the blocks,
the drops
if fire: createFire(positions)                         // 2.7
return positions.size()
```

### 2.4 `calculateExplodedPositions()` (lines 268–488)

```
set = new HashSet<BlockPos>()
for i in 0..16: for j in 0..16: for k in 0..16:
    if i, j, k are all in 1..=14: continue               // the inside of the cube
    dx = (double)((float)i / 15.0f * 2.0f - 1.0f)        // float arithmetic, then f2d
    dy = (double)((float)j / 15.0f * 2.0f - 1.0f)
    dz = (double)((float)k / 15.0f * 2.0f - 1.0f)
    len = Math.sqrt(dx*dx + dy*dy + dz*dz)               // double
    dx /= len; dy /= len; dz /= len
    power = radius * (0.7f + level.random.nextFloat() * 0.6f)     // float; THE DRAW
    x, y, z = center.x, center.y, center.z
    while power > 0.0f:
        pos = BlockPos.containing(x, y, z)               // floor each
        state = level.getBlockState(pos); fluid = level.getFluidState(pos)
        if !level.isInWorldBounds(pos): break            // this ray ends
        r = calculator.getBlockExplosionResistance(this, level, pos, state, fluid)
        if r.isPresent(): power -= (r.get() + 0.3f) * 0.3f          // float
        if power > 0.0f && calculator.shouldBlockExplode(this, level, pos, state, power):
            set.add(pos)
        x += dx * 0.30000001192092896                    // 0.3f widened; double
        y += dy * 0.30000001192092896
        z += dz * 0.30000001192092896
        power -= 0.22500001f                             // float
return new ObjectArrayList(set)
```

- The products `dx·dx` and so on are separate multiplications and additions (no fused
  multiply-add). `(float)i / 15.0f · 2.0f − 1.0f` for `i = 0..15` gives sixteen floats
  from −1 to 1; the ray's direction is that point of the cube's surface normalised.
- The block and fluid are read before the bounds test (a read outside the world gives
  air or void air). `isInWorldBounds` (Level.txt 161–173) is the build height and
  `|x|, |z| < 30 000 000`.
- `shouldBlockExplode` receives the power **after** this point's loss.
- The draw comes before the ray's first step and does not depend on anything read; the
  1352 draws are the explosion's first draws from the level's random, in this order.
- The set is unordered; see 2.0 item 3. Its size is the packet's `blockCount`.
- **What it reads**: block states and fluid states at the ray points, every one within
  `0.3·n` of the centre for `n` up to 23 (TNT); nothing of entities.

### 2.5 `hurtEntities()` (lines 490–751)

```
if radius < 1.0E-5f: return
f = radius * 2.0f                                                    // float
x0 = Mth.floor(center.x - (double)f - 1.0); x1 = Mth.floor(center.x + (double)f + 1.0)
(y0, y1, z0, z1 alike)
for e in level.getEntities(source, AABB(x0, y0, z0, x1, y1, z1)):    // not the source;
no spectators
    if e.ignoreExplosion(this): continue
    d = Math.sqrt(e.distanceToSqr(center)) / (double) f
    if d > 1.0: continue
    at = e is PrimedTnt ? e.position() : e.getEyePosition()
    dir = at.subtract(center).normalize()                            // ZERO below 9.999999747378752E-6
    damages = calculator.shouldDamageEntity(this, e)                 // true
    kb = calculator.getKnockbackMultiplier(e)                         // 1.0f
    seen = (damages || kb != 0.0f) ? getSeenPercent(center, e) : 0.0f
    if damages && (source == null || source.doTeamsAllowDamage(e)):
        e.hurtServer(level, damageSource, calculator.getEntityDamageAmount(this, e, seen))
    res = e is LivingEntity ? e.getAttributeValue(EXPLOSION_KNOCKBACK_RESISTANCE) : 0.0
    k = (1.0 - d) * (double) seen * (double) kb * (1.0 - res)
    push = dir.scale(k)
    e.pushFromExplosion(push)                                         // Entity: push(push): motion += push; needsSync (Entity.txt 5299–5328)
    if e.is(#redirectable_projectile) && e is Projectile:
        e.setOwner(damageSource.getEntity())                          // N9
    else if e is Player && !e.isSpectator() && (!e.isCreative() || !e.getAbilities().flying):
        hitPlayers.put(e, push)
    e.onExplosionHit(source)                                          // ServerPlayer: notes the impact (fall damage of wind charges)
```

- `getEntities(except, box)` (EntityGetter.txt 19–26) uses `NO_SPECTATORS`; the order is
  the entity sections' (`LevelEntityGetter` is not dumped). The order matters only for
  which of two entities' deaths drops first; each entity is hurt and pushed once.
- The box is in whole blocks: for TNT at `(x, y, z)` from `floor(x − 9)` to
  `floor(x + 9)`; the distance test (`d ≤ 1`, i.e. within `2r` = 8 of the feet) is the
  real limit.
- `doTeamsAllowDamage` (Entity.txt 7029–7049): no team on the source → true.
- `push(Vec3)` adds only finite values and sets `needsSync`, which sends the entity's
  motion (`SetEntityMotion`) on its next update (N5 brief 1.11.1).
- The knockback is scaled by exposure and distance but not by the damage; an entity that
  takes no damage (a creative player, a primed TNT) is pushed all the same.

**`getSeenPercent(center, entity)`** (lines 87–254), all `double`:

```
b = entity.getBoundingBox()
sx = 1.0 / ((b.maxX - b.minX) * 2.0 + 1.0)
sy = 1.0 / ((b.maxY - b.minY) * 2.0 + 1.0)
sz = 1.0 / ((b.maxZ - b.minZ) * 2.0 + 1.0)
ox = (1.0 - Math.floor(1.0 / sx) * sx) / 2.0
oz = (1.0 - Math.floor(1.0 / sz) * sz) / 2.0
if sx < 0 || sy < 0 || sz < 0: return 0.0f
seen = 0; total = 0
for (a = 0.0; a <= 1.0; a += sx):                      // accumulated by addition
  for (c = 0.0; c <= 1.0; c += sy):
    for (g = 0.0; g <= 1.0; g += sz):
      p = Vec3(Mth.lerp(a, b.minX, b.maxX) + ox, Mth.lerp(c, b.minY, b.maxY), Mth.lerp(g, b.minZ, b.maxZ) + oz)
      if level.clip(ClipContext(p, center, COLLIDER, Fluid.NONE, entity)).getType() == MISS: seen++
      total++
return (float) seen / (float) total
```

- `Mth.lerp(t, a, b)` = `a + t·(b − a)`. There is no offset in `y`. The loops add the
  step each time (`a += sx`), so the number of samples depends on the rounding of the
  sums; port it as additions, never as `i·sx`. A TNT (0.98 wide): `sx = 1/2.96`,
  samples at `a = 0, 0.338, 0.676` (the fourth sum passes 1): 3 × 3 × 3 = 27 rays.
- `Level.clip` with `COLLIDER` uses the blocks' collision shapes as seen by the entity
  (`ClipContext.Block.COLLIDER` with the entity's collision context: scaffolding and
  powder snow depend on it) and ignores fluids. Clustine has the walk
  (`physics::walk::traverse`) and a clip of this kind in `region/falling.rs` (with
  `SOURCE_ONLY` there).
- The clip runs from each sample point **to** the centre: the blocks read are those
  between the entity and the explosion, up to 9 blocks away for TNT.

**What hurts what** (the N6 entities and those before):

| Entity | `ignoreExplosion` | `hurtServer` with an explosion |
|---|---|---|
| item | no (Entity) | health 5 minus the amount (`(int)` of the float difference, `physics/item.rs` `hurt`); 0 or less destroys it; a stack whose `damage_resistant` is `#is_explosion` (the nether star) is untouched (**Inferred**: `ItemEntity.hurtServer` is not in these dumps; N4 ported its fire branch) |
| primed TNT | no | refuses (false); pushed from its feet |
| falling block | no | **Inferred**: refuses (not dumped); pushed |
| minecart, boat | `VehicleEntity`: true only when the cause is a `Mob` and `mob_griefing` is off | `damage += amount·10`; > 40 breaks it with its item, unless the cause (the TNT's owner) is a creative player, which discards it at any amount (N5 brief 3.14); a TNT minecart primes (1.6) |
| armour stand | `!shouldAffectBlocklikeEntities() ‖ isInvisible()` (ArmorStand.txt 1175–1185) | `#is_explosion` → `brokenByAnything` (its equipment drops, not the stand), `kill` (N5 brief 5.4, rule 5) |
| item frame, painting, knot, cushion | `BlockAttachedEntity`: true if the direct source is in water; else `!shouldAffectBlocklikeEntities()` (BlockAttachedEntity.txt 193–213) | broken at any amount; the item dropped unless the cause is a creative player (N5 brief section 6, 5.8) |
| player | no | creative: `abilities.invulnerable` refuses (Player.txt 1492–1510); survival: difficulty scaling (N7) |

A TNT in water ignores frames and paintings; it still hurts and pushes everything else
(the entities' part does not care about water), but its rays stop at once in the water
(resistance 100) and break nothing.

### 2.6 `interactWithBlocks(positions)` (lines 753–804)

```
drops = new ArrayList<StackCollector>()
Util.shuffle(positions, level.random)                    // n − 1 draws: nextInt(n), …,
nextInt(2)
for pos in positions:
    level.getBlockState(pos).onExplosionHit(level, pos, this, (stack, p) -> addOrAppendStack(drops, stack, p))
for c in drops:
    Block.popResource(level, c.pos, c.stack)              // N4 brief 8.4: three level-random draws, then the item
```

`Util.shuffle(list, random)` (Util.txt 1957–1987): `for i = size; i > 1; i--: j =
random.nextInt(i); swap(list[i−1], list[j])`.

**`BlockBehaviour.onExplosionHit(state, level, pos, explosion, onDrop)`** — not dumped;
**Inferred** from 1.21, to be read:

```
if state.isAir() || explosion.getBlockInteraction() == TRIGGER_BLOCK: return
block = state.getBlock()
byPlayer = explosion.getIndirectSourceEntity() instanceof Player
if block.dropFromExplosion(explosion):                     // true but for TNT (and a few
others)
    blockEntity = state.hasBlockEntity() ? level.getBlockEntity(pos) : null
    params = LootParams(level)
        .with(ORIGIN, Vec3.atCenterOf(pos)).with(TOOL, ItemStack.EMPTY)
        .withOptional(BLOCK_ENTITY, blockEntity).withOptional(THIS_ENTITY, explosion.getDirectSourceEntity())
    if explosion.getBlockInteraction() == DESTROY_WITH_DECAY:
        params.with(EXPLOSION_RADIUS, explosion.radius())
    state.spawnAfterBreak(level, pos, ItemStack.EMPTY, byPlayer)     // ores' experience, infested blocks' silverfish
    for stack in state.getDrops(params): onDrop.accept(stack, pos)
level.setBlock(pos, AIR, 3)
block.wasExploded(level, pos, explosion)
```

- Blocks that override it for `canTriggerBlocks` (buttons, levers, bells, doors and the
  like, for wind charges) do nothing different for TNT (**Inferred**).
- The set is flag 3 (neighbours and clients): each block's removal updates its
  neighbours and their shapes before the next block of the shuffled list is handled, so
  sand above falls, water flows in, torches pop (their own drops, by `popResource`, at
  once and not merged), and a block that a neighbour's update has already removed is
  air when its turn comes (skipped). Clustine's updates do this through `updates.rs`.
- A block with a container (chest, barrel, dispenser, hopper, …) spills its contents as
  it is set to air: `BlockEntity.preRemoveSideEffects` → `Containers.dropContents(level,
  pos, container)` (BlockEntity.txt 557–575; N5 brief 3.18 for the draws: per slot three
  `nextDouble` of the level's random, then per part `nextInt(21)` and the item's
  `triangle`s). These items are laid at once, before the collected drops. A shulker box
  overrides it (ShulkerBoxBlockEntity.txt 342–) and keeps its contents in its item
  (`copy_components`), which Clustine's loot cannot carry (`clustine_blocks::loot`):
  **an exploded shulker box's contents would be lost** unless N6 gives items their
  components (section 3's matter).
- `wasExploded` is `TntBlock`'s (1.2) for TNT; other blocks' are not read (**Inferred**:
  none of the blocks a creative player meets does anything there).
- **`addOrAppendStack(drops, stack, pos)`** (lines 887–915): for each collector in order,
  `tryMerge(stack)`; stop as soon as `stack` is empty; if any is left, a new collector
  `(pos, stack)` at the end.
- **`StackCollector.tryMerge(other)`** (StackCollector.txt 19–33): if
  `ItemEntity.areMergable(this.stack, other)` (`other.count + this.count ≤
  other.maxStackSize` and the same item and components, N4 brief 3.4), `this.stack =
  ItemEntity.merge(this.stack, other, 16)`: `moved = min(min(this.max, 16) −
  this.count, other.count)`, `this.stack` grows by `moved`, `other` shrinks by it. A
  collector that already holds more than 16 (a block's single drop above 16; none of the
  block tables gives that) would give items back (`moved` negative): port the formula
  as it is.
- `MAX_DROPS_PER_COMBINED_STACK = 16` (line 5).

### 2.7 `createFire(positions)` (lines 806–847)

Not for TNT (fire is false); for completeness (beds, respawn anchors, ghast fireballs):
for each position in the shuffled order, `level.random.nextInt(3) == 0` (drawn for every
position first) and the block now air and the block below `isSolidRender` →
`setBlockAndUpdate(pos, BaseFireBlock.getState(level, pos))`.

### 2.8 The packet and what the client does

`ClientboundExplodePacket` (clientbound `explode`, 36; ClientboundExplodePacket.txt
3–49, codec 138–162) carries the centre, radius, block count, the player's knockback, the
particle, the sound, the block particles and whether to play the sound (section 9 field
by field). **Client**: the client plays the sound (with its own random pitch), shows the
large or small particle at the centre, the block particles from `blockCount` and the
weighted list, and adds the knockback to its own motion. No block change goes in it:
the blocks reach the client by the ordinary block updates of each `setBlock`.

### 2.9 What exists in Clustine to reuse

- `region/drops.rs`: `drop_resources` and `pop_resource_at` lay a block's loot as
  `Block.popResource` does (three position draws, then the item's own), with the block's
  stream; the explosion needs the loot with `THIS_ENTITY` present and, for decay, the
  radius (`clustine_blocks::loot`: `SurvivesExplosion` and `ExplosionDecay` today answer
  for no explosion only), then the merging of 2.6 before any `pop_resource`.
- `physics/item.rs` `Item::hurt`: the item's health; it needs an explosion source and
  the `damage_resistant` test (`clustine-data` `item_components.rs` already has
  `damage_resistant: Some("minecraft:is_explosion")` for the nether star).
- `region/vehicles.rs` `hurt_vehicle`, `living.rs` (the armour stand's damage),
  `region/attached.rs` `hurt_attached`, `region/hanging.rs`: the entities' damage paths,
  which take an explosion source and the push.
- `physics::walk::traverse` and the clip of `region/falling.rs`: the exposure's clip.
- `region/random_ticks/fire.rs`: where burning TNT is to be primed (`check_burn_out`).
- `region/updates.rs`: the updates of each `setBlock(…, 3)`.
- `clustine_noise` (`libm`): the priming's `sin`/`cos`.

## 3. Containers that open

This half of the section is about the **blocks** (and the container vehicles) that open
a menu: what a click on them does, which menu they make and under what title, the
openers' count with its sounds, block events, game events and scheduled rechecks, the
shulker box's lid, what they keep and save, and the comparator's reading of them. The
menus themselves (slots, every click type, the synchroniser, the state id, hashed
stacks, closing, the packets of the menu) begin at 3.10.

Classes read: `ChestBlock` (and `$1`, `$2`, `$2$1`; ChestBlock.txt, 728 lines),
`ChestBlockEntity` (and `$1`), `ChestLidController`, `ContainerOpenersCounter`,
`BarrelBlock`, `BarrelBlockEntity` (and `$1`), `ShulkerBoxBlock`, `ShulkerBoxBlockEntity`
(and `$AnimationStatus`), `BaseContainerBlockEntity`, `RandomizableContainerBlockEntity`,
`BlockEntity`, `Container`, `CompoundContainer`, `SimpleContainer`, `ContainerHelper`,
`Containers`, `ComparatorBlock`, the use and analog methods of `DispenserBlock`,
`HopperBlock` and `CrafterBlock`, the constructors and `removed` of the five menus;
from the N5 dump `ServerPlayer.openMenu`, `ServerPlayerGameMode.useItemOn`,
`Player.hasContainerOpen`/`isWithinBlockInteractionRange`, `Level.playSound`,
`Level.updateNeighbourForOutputSignal`, `CollisionGetter.noCollision`, `Mth.lerpDiscrete`,
`ContainerEntity`, `MinecartChest`, `AbstractChestBoat`. Ids are from
`generated/reports/registries.json`.

### 3.0 Findings of this section

1. **A click on a chest, barrel, shulker box, dispenser, dropper, hopper or crafter is
   the block's `useWithoutItem`**, reached from `ServerPlayerGameMode.useItemOn`
   (ServerPlayerGameMode.txt 1029–1171) whenever the player is not sneaking with
   something in a hand: `BlockState.useItemOn` answers `TRY_WITH_EMPTY_HAND` for these
   blocks (none overrides `useItemOn`; **Inferred** from `BlockBehaviour`, not read), and
   the main hand then runs `useWithoutItem`. Every one answers `SUCCESS` on the server
   whatever happens, so **the click is consumed even when nothing opens** (a blocked chest,
   a shulker box that cannot open, a locked container): no block is placed against it.
   Today `placement.rs` (`is_used`, `use_block`) already treats these blocks as used and
   does nothing; N6 replaces that nothing with opening a menu.
2. **Opening is `ServerPlayer.openMenu(provider)`** (ServerPlayer.txt 3191–3248, N5 dump),
   and its order matters to the packets: close the open menu if any (not the inventory)
   → `containerCounter = containerCounter % 100 + 1` (3179–3189; 1 to 100, advanced even
   when nothing opens) → `provider.createMenu(counter, inventory, player)`, whose menu
   constructor calls `container.startOpen(player)` (the openers' count, the sound, the
   block event, the game event) → if null: a spectator is told
   `container.spectatorCantOpen` (red, overlay), and nothing else → else
   `ClientboundOpenScreenPacket(containerId, menu type, provider.getDisplayName())` →
   `initMenu(menu)` (the synchroniser's first full send, 3.10 on) → `containerMenu = menu`.
3. **The menus and titles** (`minecraft:menu` protocol ids): chest, trapped chest, copper
   chests and barrel `generic_9x3` (2); a double chest `generic_9x6` (5); shulker boxes
   `shulker_box` (20); dispenser and dropper `generic_3x3` (6); hopper `hopper` (16);
   crafter `crafter_3x3` (7). The title is the custom name if set, else the translatable
   `container.chest`, `container.chestDouble`, `container.barrel`, `container.shulkerBox`,
   `container.dispenser`, `container.dropper`, `container.hopper`, `container.crafter`. A
   double chest takes the first half's custom name, else the second's, else
   `container.chestDouble` (3.3).
4. **A double chest is a `CompoundContainer(first, second)` where `first` is the half
   whose `type` is `right`**: slots 0–26 are that half's, 27–53 the `left` half's.
   `ChestBlock.getBlockType`: `right` → `FIRST`, `left` → `SECOND` (ChestBlock.txt 56–75).
   Both halves count openers, queue block events, schedule rechecks and send game events
   of their own; only the `right` (or single) half plays the sound, at the double chest's
   centre (3.3, 3.4).
5. **A chest with a redstone conductor above it (or a cat sitting on it) opens nothing
   and reads 0 to a comparator.** `getMenuProvider` and `getAnalogOutputSignal` both call
   `combine(…, false)`, whose test is `isChestBlockedAt` (the block above either half
   `isRedstoneConductor`, or a sitting cat in the block-high box above). Hoppers ignore
   it (`combine(…, true)`; the hopper's section). The cat is N7's; until then the test is
   the block above only.
6. **The openers' count is the game's, and it rechecks itself every 5 ticks.** Chests and
   barrels keep a `ContainerOpenersCounter`: the first opener plays the open sound, sends
   `CONTAINER_OPEN`, and schedules a block tick in 5 ticks; the chest's block tick
   (`ChestBlock.tick`, `BarrelBlock.tick`) counts again **the players within the block's
   box inflated by the largest opener's interaction range + 4 whose open menu is a
   `ChestMenu` over this container** (or a `CompoundContainer` holding it), corrects the
   count (with the close sound and `CONTAINER_CLOSE` if it fell to 0), and schedules the
   next recheck while anyone is left. So an opened chest is a scheduled tick every 5
   ticks in the region's tick list, and **a player who has it open from another region is
   not counted there**: the lid closes 5 ticks after opening (3.4; for section 11).
7. **The chest's lid is the client's.** On the server the chest's openers change only a
   block event (`blockEvent(pos, block, 1, count)`); `ChestLidController` ticks only on
   the client (`ChestBlock.getTicker` gives a ticker only `isClientSide`). The barrel has
   no block event: its `open` property is set with `setBlockAndUpdate` (flags 3) on the
   first open and the last close.
8. **The shulker box's lid is the server's too, and moves entities.** Its block event
   (`1`, openers) runs `triggerEvent` on the server as well, which sets the animation to
   `OPENING` (count 1) or `CLOSING` (count 0); its ticker (on both sides,
   `ShulkerBoxBlock.getTicker` has no side test) moves `progress` by 0.1 a tick, updates
   its neighbours' shapes and neighbours when it starts and ends (its support shape
   changes), and while opening moves every entity in the swept band (`MoverType
   .SHULKER_BOX`) out of the lid's way. Its collision shape grows with the progress
   (`getShape` from the block entity's box). The progress is not saved: a box loaded is
   closed (3.5).
9. **A shulker box opens only if closed with room for its lid**: when `CLOSED`, the half
   block beyond its facing face (deflated by 1e-6) must be free of block collisions,
   colliding entities and the world border (`Level.noCollision(AABB)`), else the click is
   consumed and nothing opens; any other status opens at once (3.5).
10. **Breaking a container spills its contents, in creative too.** `BlockEntity
    .preRemoveSideEffects` (BlockEntity.txt 557–576) calls `Containers.dropContents` for
    every block entity that is a `Container` when its block is removed (**Inferred**: by
    `LevelChunk.setBlockState` unless the flags skip block entity side effects; not read),
    whoever breaks it: the N4 rule "a creative player's break drops nothing" covers the
    block's own loot only. Each slot drops as items of 10 to 30 at a time with two
    `triangle` draws a stack (3.6, section 10). A chest with an unopened loot table
    unpacks it first (`getItem` unpacks), so **breaking a structure's chest spills its
    loot**. The shulker box overrides `preRemoveSideEffects` to nothing (its contents go
    in its item), and a creative player breaking a non-empty shulker box gets the box as
    an item with its contents (`playerWillDestroy`, 3.5).
11. **Loot tables are unpacked at the first touch, not at generation.** Chests, barrels,
    dispensers, hoppers and shulker boxes placed by structures keep `LootTable` (and
    `LootTableSeed`) in their data, which Clustine's structures already write
    (`crates/clustine-structures/src/resave.rs`). Opening one, reading it through a
    comparator (`getItem`), a hopper pulling, or breaking it, runs `unpackLootTable`:
    the table filled into the container with the seed's random (**Inferred**: the
    `RandomizableContainer` interface and `LootTable.fill` are not read). Clustine has
    only block loot tables (`clustine-data`'s `loot_tables.rs`: 1201 `blocks/…`, no
    `chests/…`); the 31 `chests/…`, `barrels/…` and `dispensers/…` tables are in
    `generated/data/minecraft/loot_table/`. A spectator cannot open a container with a
    loot table still set.
12. **Locks** (`BaseContainerBlockEntity.canOpen`: `lockKey.canUnlock(player)`): a
    container with a `lock` component opens only for a player whose main hand matches
    its predicate (**Inferred**: `LockCode` not read). A refused player is sent
    `container.isLocked` with the container's name as an overlay message, and
    `block.chest.locked` (348) plays at the block's centre at volume and pitch 1. Only
    `/data` or an item with the component makes a lock; a creative player meets one in a
    map that has one.
13. **Copper chests, trapped chests and the ender chest are chests of their own
    classes**, not read: the copper chests (eight blocks, `CopperChestBlock` and
    `WeatheringCopperChestBlock`) share the `chest` block entity type (BlockEntityTypes
    .txt 171–178) and differ in their sounds (`block.copper_chest*.open/close`) and, by
    **Inference**, in joining any copper chest; the trapped chest (`trapped_chest`
    block entity, 2) gives a redstone signal of its openers' count; the ender chest opens
    the player's own ender inventory (`generic_9x3`, `container.enderchest`), which N2's
    player data does not keep yet (section 12).
14. **No container block sends `block_entity_data`.** `BlockEntity.getUpdatePacket` is
    null and `getUpdateTag` an empty compound (BlockEntity.txt 525–535), and none of these
    block entities overrides either: a client learns nothing of a container's contents
    or name except through a menu. The chunk packet lists them with an empty tag.

#### Findings of 3.10 to 3.21 (menus and clicks)

Items 15 to 26 are the findings of 3.10 to 3.21 (the menus, the clicks and the
synchroniser); items 1 to 14 above are those of the container blocks.

15. **`ClickType` is now `ContainerInput`** (`net.minecraft.world.inventory.ContainerInput`,
   not dumped). `AbstractContainerMenu.clicked(slot, button, ContainerInput, player)` names
   its seven values `PICKUP`, `QUICK_MOVE`, `SWAP`, `CLONE`, `THROW`, `QUICK_CRAFT`,
   `PICKUP_ALL`; the wire ids 0 to 6 in that order are **Inferred** (the enum is not read;
   SteelMC's 26.3 reader agrees). `ClickAction` (`PRIMARY` for button 0, `SECONDARY` for 1)
   picks left or right within `PICKUP` and `QUICK_MOVE` (3.14).
16. **A click whose state id is stale is still carried out.** `handleContainerClick`
   (SGPLI 5900–6052) compares the packet's `stateId` with the menu's only to choose, after
   the click, between `broadcastFullState` (stale: everything sent again) and
   `broadcastChanges` (current: only what the client got wrong). The click itself runs
   either way. Refused without any answer: a `containerId` that is not the open menu's, a
   menu no longer `stillValid`, a slot index outside `-999`, `-1`, `0..slots.size()-1`. A
   spectator or a dying player gets `sendAllDataToRemote` and nothing changes (3.17).
17. **The client predicts, the server checks by hashes.** The click packet carries the
   client's view of every slot it changed and of the carried stack as `HashedStack`s (an
   item, a count and a CRC of each component, not the components). The server writes those
   into its *remote* slots (what it believes the client shows), runs the click with
   sending suppressed, and then `broadcastChanges` sends `container_set_slot` only for a
   slot whose real stack does not match the client's hash, and `set_cursor_item` only if
   the carried stack does not (**Inferred**: the synchroniser `ServerPlayer$1` is not
   read). A client that predicted right is sent nothing (3.15, 3.16).
18. **Clustine's stacks have no components** (`clustine_world::entity::ItemStack` is `item`
   and `count`). For the menus that is enough to compare stacks the game's way
   (`isSameItemSameComponents` reduces to the item) and to match a client's hash whose
   component maps are empty. A client hash with components (a named item, a written book,
   a shulker box with contents, a stack the creative tab gave with components) never
   matches, so the server resends that slot after every click; harmless, but the stacks the
   creative tab gives (with `set_creative_mode_slot`, whose components the edge drops today)
   lose their components. Shulker boxes keep their contents in the `minecraft:container`
   component when broken and picked; that belongs to 3.1–3.9 but needs components in
   `ItemStack` (and the CRC32C of `HashOps` to match hashes) before a box can carry
   anything.
19. **What the edge owns and what the region decides** (ADR-0005). The click's rules read
   and write the container and the player's inventory together, so they are the region's
   (the holder of the player and the container), as `PlayerInput`s; the remote slots, the
   hashed stacks, the state id, the container id counter and every packet are the edge's.
   The `ContainerSynchronizer` interface (ContainerSynchronizer.txt) is exactly that seam:
   `sendInitialData(menu, items, carried, data)`, `sendSlotChange(menu, slot, stack)`,
   `sendCarriedChange(menu, stack)`, `sendDataChange(menu, id, value)` and `createSlot()`.
   Proposal: the region reports, after each click and at each player tick, the menu's slots
   and carried stack (as an event of the player, as `PlayerEvent::InventorySlot` is now);
   the edge keeps the remote slots (stacks it sent, or hashes the client reported),
   compares,
   and sends. The region never sees a hash or a state id (3.21).
20. **Only the open menu is synchronised.** `ServerPlayer.tick` (ServerPlayer.txt 939–951)
   calls `containerMenu.broadcastChanges()` and then closes the menu if it is no longer
   `stillValid`; `doTick` (1085–1094) checks `stillValid` again after `Player.tick`. While a
   chest is open the player's inventory reaches the client through the chest menu's slots
   (`container_set_slot` with the chest's container id), not through `inventoryMenu`;
   closing copies what the client was told of the shared slots into `inventoryMenu`
   (`transferState`, AbstractContainerMenu.txt 2310–2404) so nothing is sent again. The edge
   sends `set_player_inventory` today for every slot the region changes; that still works
   for the client while a menu is open (**Client**), but the edge must then update its
   remote record of the open menu's slot as well, or the next comparison resends it.
21. **The container id runs 1 to 100** (`nextContainerCounter`, ServerPlayer.txt 3179–3189:
   `counter = counter % 100 + 1`), 0 being the player's own inventory menu. `openMenu`
   closes an open menu first with `container_close` of its id (3191–3248). The counter is
   the connection's, so the edge's.
22. **`stillValid` of every N6 menu is its container's** (`Container.stillValid`): for a
    block
   entity `Container.stillValidBlockEntity(be, player, 4.0f)` (Container.txt 116–149): the
   level's block entity at its position is still *this one* (by identity) and
   `player.isWithinBlockInteractionRange(pos, 4.0)`: the squared distance from the eyes to
   the block's unit box is below `(blockInteractionRange + 4.0)²` (Player.txt 4883–4905).
   So a menu closes on the next player tick after its block is broken or replaced, or when
   its player is about 8.5 blocks away (9 in creative, **Inferred**: the creative bonus of
   the attribute is not read). A click in between is refused silently (item 2).
23. **The crafter's slot toggles are not clicks**: `container_slot_state_changed`
   (serverbound 20) reaches `CrafterBlockEntity.setSlotState` (SGPLI 2824–2869) with only a
   spectator check and a container id check: no `stillValid`, no slot range check here
   (section 7). `container_button_click` (17) calls `clickMenuButton`, which is `false` for
   every N6 menu (AbstractContainerMenu.txt 810–813): ignore it.
24. **`set_creative_mode_slot` reaches slots 1 to 45 of the inventory menu** (SGPLI
    6226–6329), the crafting grid's 1 to 4 included, even while another menu is open; the
    edge keeps only 5 to 45 today (`Inventory::from_container`). The 2×2 grid's items live
    on the server in the game and come back to the inventory when the inventory menu is
    "closed" (not read: `InventoryMenu`). A small named difference, or the grid kept.
25. **Malformed clicks crash the game's handler**; the port refuses them instead.
    `isValidSlotIndex` lets `-999` and `-1` through, and `SWAP` and `QUICK_CRAFT`'s
    continue (`status 1`) then call `slots.get(slot)` with no check, which throws (the
    click's `clicked` wraps it in a `ReportedException`, 847–910). Every other negative slot
    is a no-op in the game, and should be a no-op here.
26. **Nothing in a menu draws a random number** except the items thrown out of it, which
    are `Player.drop(stack, true, PREDICTED)` as the N2 Q throw is (section 10). The drag's
    slot set is a `HashSet` (iteration order by identity hash), but what each slot gets does
    not depend on the order (3.14.6).

### 3.1 Which block opens what

| Block (class) | `useWithoutItem` (lines) | Block entity (type id) | Size | Menu (id) | Default title | Opens when |
|---|---|---|---|---|---|---|
| chest, copper chests (`ChestBlock`, `CopperChestBlock`, `WeatheringCopperChestBlock`) | ChestBlock.txt 380–409 | `ChestBlockEntity` (`chest`, 1) | 27, or 54 joined | `generic_9x3` (2) / `generic_9x6` (5) | `container.chest` / `container.chestDouble` | not blocked; both halves' lock passes |
| trapped chest (`TrappedChestBlock`, not read) | inherited (**Inferred**) | `TrappedChestBlockEntity` (`trapped_chest`, 2) | as chest | as chest | as chest (**Inferred**) | as chest |
| barrel (`BarrelBlock`) | BarrelBlock.txt 29–59 | `BarrelBlockEntity` (`barrel`, 26) | 27 | `generic_9x3` (2) | `container.barrel` | lock passes |
| shulker boxes, 17 (`ShulkerBoxBlock`) | ShulkerBoxBlock.txt 50–86 | `ShulkerBoxBlockEntity` (`shulker_box`, 24) | 27 | `shulker_box` (20) | `container.shulkerBox` | `canOpen` (3.5) and lock |
| dispenser (`DispenserBlock`) | DispenserBlock.txt 61–89 | `DispenserBlockEntity` (`dispenser`, 5) | 9 | `generic_3x3` (6) | `container.dispenser` | lock passes |
| dropper (`DropperBlock` extends `DispenserBlock`) | inherited | `DropperBlockEntity` (`dropper`, 6) | 9 | `generic_3x3` (6) | `container.dropper` | lock passes |
| hopper (`HopperBlock`) | HopperBlock.txt 189–212 | `HopperBlockEntity` (`hopper`, 18) | 5 | `hopper` (16) | `container.hopper` | lock passes |
| crafter (`CrafterBlock`) | CrafterBlock.txt 271–291 | `CrafterBlockEntity` (`crafter`, 42) | 9 | `crafter_3x3` (7) | `container.crafter` | lock passes |

Each `useWithoutItem`, on the server only:

```
chest:   provider = getMenuProvider(state, level, pos)            // 3.3; null when
blocked or no block entity
         if provider != null:
             player.openMenu(provider)
             player.awardStat(Stats.CUSTOM, OPEN_CHEST)           // the trapped chest: TRIGGER_TRAPPED_CHEST (Inferred)
             PiglinAi.angerNearbyPiglins(level, player, true)     // N7 or later
         return SUCCESS
barrel:  if level.getBlockEntity(pos) is BarrelBlockEntity be:
             player.openMenu(be); awardStat(OPEN_BARREL); angerNearbyPiglins(…, true)
         return SUCCESS
shulker: if be is ShulkerBoxBlockEntity && canOpen(state, level, pos, be):     // 3.5
             player.openMenu(be); awardStat(OPEN_SHULKER_BOX); angerNearbyPiglins(…, true)
         return SUCCESS
dispenser/dropper: if be is DispenserBlockEntity (the dropper's is a subclass):
             player.openMenu(be); awardStat(be is DropperBlockEntity ? INSPECT_DROPPER : INSPECT_DISPENSER)
         return SUCCESS
hopper:  if be is HopperBlockEntity: openMenu(be); awardStat(INSPECT_HOPPER)
         return SUCCESS
crafter: if be is CrafterBlockEntity: openMenu(be)                // no stat
         return SUCCESS
```

Statistics and piglins are not Clustine's yet and can be left out (statistics are
named in `ServerPlayer` data no step keeps).

**Spectators** take another path (ServerPlayerGameMode.txt 1047–1062): a spectator's
click on any block asks `state.getMenuProvider(level, pos)` and, if not null, opens it
and answers `CONSUME`, without the block's `useWithoutItem` (no statistic, no piglins).
For a chest that is the combiner's provider (3.3); for the others `BaseEntityBlock`'s,
the block entity when it is a `MenuProvider` (**Inferred**, `BaseEntityBlock` not read).
A spectator never counts as an opener (`startOpen` and `stopOpen` return at once for a
spectator, in each block entity), so no lid moves, no sound plays and no game event is
sent; a spectator cannot open a container whose loot table is still set
(`RandomizableContainerBlockEntity.canOpen`, lines 96–111), and is then told
`container.spectatorCantOpen` by `openMenu`. Clustine's players are all in creative
today; the spectator path is the plan's to take or leave.

### 3.2 From the provider to the menu: `createMenu`, locks and loot

`BaseContainerBlockEntity.createMenu(id, inventory, player)` (lines 221–240) and the
override in `RandomizableContainerBlockEntity` (lines 113–139), which every container
here extends:

```
RandomizableContainerBlockEntity.createMenu(id, inv, player):
    if canOpen(player):
        unpackLootTable(inv.player)                 // 3.6
        return createMenu(id, inv)                  // the kind's menu; its constructor calls startOpen(player)
    if !player.isSpectator():
        sendChestLockedNotifications(Vec3.atCenterOf(getBlockPos()), player, getDisplayName())
    return null

canOpen(player) (lines 96–111):
    if lootTable != null && player.isSpectator(): false
    else BaseContainerBlockEntity.canOpen(player) = lockKey.canUnlock(player)    // lines 78–84

sendChestLockedNotifications(at, player, name) (lines 86–117):
    player.sendOverlayMessage(Component.translatable("container.isLocked", name))   // system chat, overlay true (ServerPlayer.txt 4531–4537)
    if server: level.playSound(null, at.x, at.y, at.z, CHEST_LOCKED, BLOCKS, 1.0f, 1.0f)
```

The kinds' `createMenu(id, inventory)`: chest and barrel `ChestMenu.threeRows(id, inv,
this)` (ChestBlockEntity.txt 308–315, BarrelBlockEntity.txt 88–93); shulker box `new
ShulkerBoxMenu(id, inv, this)` (ShulkerBoxBlockEntity.txt 565–572); dispenser and dropper
`new DispenserMenu(id, inv, this)` (DispenserBlockEntity.txt 195); hopper `new
HopperMenu(id, inv, this)` (HopperBlockEntity.txt 1124); crafter `new CrafterMenu(id,
inv, this, this)` with its data slots (CrafterBlockEntity.txt 66; the crafter's section).
Each menu constructor calls `container.startOpen(inventory.player)` (ChestMenu.txt 133,
ShulkerBoxMenu.txt 34, DispenserMenu.txt 42, HopperMenu.txt 34, CrafterMenu.txt 82), and
its `removed(player)` calls `container.stopOpen(player)` (ChestMenu.txt 271–279,
ShulkerBoxMenu.txt 152–160, DispenserMenu.txt 170–178, HopperMenu.txt 133–141). So the
openers' count follows the menu's life exactly: opened by the constructor, closed by
`removed` (closing, disconnecting, opening another menu, death: 3.10 on).

`Container.startOpen` and `stopOpen` are empty by default (Container.txt 35–41): the
dispenser, dropper, hopper and crafter count nobody, play no sound and send no game
event when opened.

### 3.3 Chests, single and double (ChestBlock.txt, ChestBlockEntity.txt)

**State.** `facing` (horizontal, default north), `type` (`single`, `left`, `right`),
`waterlogged`. Placement (`getStateForPlacement`, 220–303) and `updateShape` (76–152)
are N1's and built; restated for the combiner: a half's partner lies at
`getConnectedDirection(state)` = `facing.getClockWise()` for `left`,
`facing.getCounterClockWise()` for `right` (191–208). Two chests join only when the
neighbour is the same block (`chestCanConnectTo`: `state.is(this)`, 154–159; the copper
chests override it, **Inferred**).

**The combiner** (`combine(state, level, pos, ignoreBlocked)`, 442–463):
`DoubleBlockCombiner.combineWithNeigbour(blockEntityType, ChestBlock::getBlockType,
ChestBlock::getConnectedDirection, FACING, state, level, pos, blockedTest)`, the test
being always false when `ignoreBlocked` and `isChestBlockedAt` otherwise (**Inferred**
which `invokedynamic` is which: `lambda$combine$0` returns false, lines 693–697, and is
the first). `DoubleBlockCombiner` is not read; **Inferred** from its source:

```
be = level's block entity at pos of the chest type;  none → NONE
if blockedTest(level, pos) → NONE
kind = getBlockType(state)                 // SINGLE, FIRST (right), SECOND (left)
if kind == SINGLE → SINGLE(be)
partner = pos.relative(getConnectedDirection(state)); ps = its state
if ps is the same block && getBlockType(ps) != SINGLE && != kind && ps.facing ==
state.facing:
    if blockedTest(level, partner) → NONE
    pbe = block entity at partner; if pbe != null:
        → DOUBLE(kind == FIRST ? be : pbe, kind == FIRST ? pbe : be)
→ SINGLE(be)
```

`CHEST_COMBINER` (`ChestBlock$1`): double → `new CompoundContainer(first, second)`;
single → the block entity; none → empty. `MENU_PROVIDER_COMBINER` (`ChestBlock$2`):
double → a `MenuProvider` over `CompoundContainer(first, second)` (`ChestBlock$2$1`);
single → the block entity; none → empty. `getContainer(block, state, level, pos,
ignoreBlocked)` (426–440) and `getMenuProvider` (465–479, `ignoreBlocked = false`) take
`orElse(null)`.

**The double chest's provider** (`ChestBlock$2$1`):

```
createMenu(id, inv, player):
    if first.canOpen(player) && second.canOpen(player):          // RandomizableContainerBlockEntity.canOpen each
        first.unpackLootTable(inv.player); second.unpackLootTable(inv.player)
        return ChestMenu.sixRows(id, inv, compound)              // its constructor: compound.startOpen → first, then second
    if !player.isSpectator():
        dir = ChestBlock.getConnectedDirection(first.getBlockState())
        at  = Vec3.atCenterOf(first.getBlockPos()).add(dir.getStepX() / 2.0, 0.0, dir.getStepZ() / 2.0)
        BaseContainerBlockEntity.sendChestLockedNotifications(at, player, getDisplayName())
    return null
getDisplayName():
    first.hasCustomName() ? first.getDisplayName()
  : second.hasCustomName() ? second.getDisplayName()
  : Component.translatable("container.chestDouble")
```

**Blocked** (`isChestBlockedAt`, 512–525): `isBlockedChestByBlock` (527–538: the block at
`pos.above()` `isRedstoneConductor` there) or `isCatSittingOnChest` (540–595:
`getEntitiesOfClass(Cat, AABB(x, y + 1, z, x + 1, y + 2, z + 1))`, any
`isInSittingPose()`). For a double chest each half is tested (the combiner's two tests).

**`ChestBlockEntity`** (326 lines): 27 slots (`getContainerSize`, 47–50); default name
`container.chest` (static, 324); `startOpen`/`stopOpen` (190–234) go to its counter
unless removed or the user is a spectator; `triggerEvent(1, n)` sets the lid controller's
`shouldBeOpen(n > 0)` and answers true, any other event goes to the superclass (169–188);
`signalOpenCount(level, pos, state, old, new)` = `level.blockEvent(pos, state.getBlock(),
1, new)` (332–345); `recheckOpen()` (316–330) runs the counter's `recheckOpeners` unless
removed; `getOpenCount(getter, pos)` (268–292) is the counter's count where the block
there has a `ChestBlockEntity`, else 0 (the trapped chest's signal, **Inferred**);
`swapContents` (294–306) swaps two chests' lists (used by nothing read). Its counter
(`ChestBlockEntity$1`): `onOpen`/`onClose` play the block's `getOpenChestSound`/
`getCloseChestSound` (the constructor's, so the copper chests' own) through `playSound`;
`openerCountChanged` → `signalOpenCount`; `isOwnContainer(player)`: `player.containerMenu`
is a `ChestMenu` whose container is this block entity, or a `CompoundContainer` that
`contains` it.

**The chest's sound** (`playSound(level, pos, state, sound)`, ChestBlockEntity.txt
100–167):

```
type = state.type
if type == LEFT: return                                  // the left half is silent: no draw
x = pos.x + 0.5; y = pos.y + 0.5; z = pos.z + 0.5        // double
if type == RIGHT:
    d = ChestBlock.getConnectedDirection(state)
    x += d.getStepX() * 0.5; z += d.getStepZ() * 0.5     // the double chest's centre
level.playSound(null, x, y, z, sound, BLOCKS, 0.5f, level.random.nextFloat() * 0.1f + 0.9f)
```

`Level.playSound(Entity, double…)` (Level.txt 746–761, N5 dump) also draws the sound's
seed from `soundSeedGenerator.nextLong()`, a random apart from the level's.

**Sounds by block** (`minecraft:sound_event` ids): chest and trapped chest
`block.chest.open` (349) / `block.chest.close` (347); copper chests
`block.copper_chest.open/close` (406/405), weathered (408/407), oxidized (410/409); the
exposed ones **Inferred** to use the plain copper sounds (no `exposed` sound exists);
`block.chest.locked` (348).

**The trapped chest** (`TrappedChestBlock`, `TrappedChestBlockEntity`, not read):
**Inferred** from the game's source: its `signalOpenCount` also updates the neighbours
of its position and of the block below, and it is a signal source of
`clamp(getOpenCount, 0, 15)` on every side and that strongly downwards. It is the one
container block whose openers are redstone, so the one that needs the openers' count
in the simulation for the circuit's sake.

### 3.4 The openers' count (ContainerOpenersCounter.txt, 274 lines)

State: `openCount` (int) and `maxInteractionRange` (double), neither saved (a chest loaded
has no openers; players who had it open had their menus closed when they left).

```
incrementOpeners(user, level, pos, state, range) (23–63):
    prev = openCount; openCount = prev + 1
    if prev == 0:
        onOpen(level, pos, state)                                // the open sound; the barrel's open property
        level.gameEvent(user, GameEvent.CONTAINER_OPEN, pos)
        scheduleRecheck(level, pos, state)                       // level.scheduleTick(pos, state.getBlock(), 5)
    openerCountChanged(level, pos, state, prev, openCount)       // the chest's block event
    maxInteractionRange = max(range, maxInteractionRange)

decrementOpeners(user, level, pos, state) (65–99):
    prev = openCount; openCount = prev - 1
    if openCount == 0:
        onClose(level, pos, state)
        level.gameEvent(user, GameEvent.CONTAINER_CLOSE, pos)
        maxInteractionRange = 0
    openerCountChanged(level, pos, state, prev, openCount)

getEntitiesWithContainerOpen(level, pos) (101–129):
    r = maxInteractionRange + 4.0
    level.getEntities((Entity) null, new AABB(pos).inflate(r), e -> hasContainerOpen(e, pos))
    hasContainerOpen (131–149): e is a ContainerUser, not a spectator, and e.hasContainerOpen(this, pos)
        // Player.hasContainerOpen(counter, pos) = counter.isOwnContainer(player) (Player.txt 4808–4813)

recheckOpeners(level, pos, state) (151–243):
    users = getEntitiesWithContainerOpen(level, pos)
    maxInteractionRange = 0; for u in users: maxInteractionRange = max(u.getContainerInteractionRange(), it)
    n = users.size(); prev = openCount
    if prev != n:
        if n > 0 && prev == 0: onOpen; level.gameEvent(null, CONTAINER_OPEN, pos)
        else if n == 0:        onClose; level.gameEvent(null, CONTAINER_CLOSE, pos)
        openCount = n
    openerCountChanged(level, pos, state, prev, n)                // always, even when unchanged
    if n > 0: scheduleRecheck(level, pos, state)
```

Notes for the porter:

- The recheck is a **scheduled block tick** (`ChestBlock.tick`, ChestBlock.txt 665–679;
  `BarrelBlock.tick`, BarrelBlock.txt 69–83: `be.recheckOpen()`), not a block entity
  ticker, so it goes through the region's tick list and is durable with it. A chest
  with openers is ticked every 5 ticks; each recheck queues the chest's block event
  again (`openerCountChanged` is called unconditionally), so a client is sent the
  `block_event` every 5 ticks while the chest is open (**Inferred** from the code:
  `ServerLevel.doBlockEvent` sends it whenever `triggerEvent` answers true, which the
  chest's does for event 1).
- `getContainerInteractionRange` is the player's `blockInteractionRange()` (Player.txt
  4815–4819), the attribute `block_interaction_range`: 4.5, and 5.0 in creative
  (**Inferred**: the attribute's default and the creative modifier are not read; the
  repository uses the same for entities, `uses.rs`). The recheck's box is the block's
  box inflated by 9 in creative.
- `isOwnContainer` is the kind's: the chest's (above) and the barrel's
  (`BarrelBlockEntity$1`, 49–69: a `ChestMenu` whose container is this barrel). A player
  standing in the box with another menu open, or with this container's menu open but
  outside the box, does not count.
- The game event's position is the block's centre (**Inferred**:
  `Level.gameEvent(Entity, Holder, BlockPos)` is `LevelAccessor`'s default, not read);
  `CONTAINER_OPEN` (11) and `CONTAINER_CLOSE` (10) in `minecraft:game_event`; their use
  by sculk sensors is section 8's.
- Within a double chest each half has its own counter, so both halves run all of the
  above; `CompoundContainer.startOpen` calls the first (right) half, then the second
  (CompoundContainer.txt 186–208).

**The barrel's open and close** (`BarrelBlockEntity$1`, 17–43; `BarrelBlockEntity`,
169–250):

```
onOpen:  playSound(state, BARREL_OPEN);  updateBlockState(state, true)
onClose: playSound(state, BARREL_CLOSE); updateBlockState(state, false)
openerCountChanged: nothing
updateBlockState(state, open): level.setBlockAndUpdate(worldPosition,
state.setValue(OPEN, open))   // flags 3
playSound(state, sound):
    v = state.facing.getUnitVec3i()
    x = worldPosition.x + 0.5 + v.x / 2.0  (and y, z alike)       // the centre of the facing face
    level.playSound(null, x, y, z, sound, BLOCKS, 0.5f, level.random.nextFloat() * 0.1f + 0.9f)
```

`block.barrel.open` (141), `block.barrel.close` (140). The `state` the counter passes is
the block entity's cached state as `startOpen` read it (`getBlockState()`), so a barrel
set open keeps its facing. The barrel is placed facing the player's nearest look
direction's opposite (`getStateForPlacement`, BarrelBlock.txt 150–160; built).

### 3.5 Shulker boxes (ShulkerBoxBlock.txt, ShulkerBoxBlockEntity.txt)

Seventeen blocks (`shulker_box` and sixteen colours), state `facing` (six directions,
default up; placed facing the clicked face, `getStateForPlacement` 114–123). The block
entity keeps 27 slots, `openCount`, `animationStatus` (`CLOSED` 0, `OPENING` 1, `OPENED`
2, `CLOSING` 3), `progress` and `progressOld` (floats, 0 to 1) and its colour; none but
the items, name, lock and loot table is saved.

**Can it open** (`canOpen`, ShulkerBoxBlock.txt 88–112):

```
if be.getAnimationStatus() != CLOSED: return true
box = Shulker.getProgressDeltaAabb(1.0f, state.facing, 0.0f, 0.5f,
Vec3.atBottomCenterOf(pos)).deflate(1.0E-6)
return level.noCollision(box)
```

`Shulker.getProgressDeltaAabb` is not read. **Inferred** from the game's source: the
unit box at the block, stretched in `facing` from progress 0 to 0.5 and kept to the
part beyond the block, so the half block just beyond the facing face (for `up`:
`x..x+1, y+1..y+1.5, z..z+1`). `noCollision(AABB)` (CollisionGetter.txt 45–92) is no block
collision (block shapes, not fluids), no entity that blocks (`noEntityCollision`: the
entities whose `canBeCollidedWith` is true, boats and shulkers, **Inferred**) and no
world border.

**Opening and closing** (`startOpen`, 346–407; `stopOpen`, 409–463), skipped when
removed or for a spectator:

```
startOpen(user):
    if openCount < 0: openCount = 0
    openCount += 1
    level.blockEvent(worldPosition, getBlockState().getBlock(), 1, openCount)
    if openCount == 1:
        level.gameEvent(user.getLivingEntity(), CONTAINER_OPEN, worldPosition)
        level.playSound(null, worldPosition, SHULKER_BOX_OPEN, BLOCKS, 0.5f, level.random.nextFloat() * 0.1f + 0.9f)
stopOpen(user):
    openCount -= 1
    level.blockEvent(worldPosition, block, 1, openCount)
    if openCount <= 0:
        level.gameEvent(user.getLivingEntity(), CONTAINER_CLOSE, worldPosition)
        level.playSound(null, worldPosition, SHULKER_BOX_CLOSE, BLOCKS, 0.5f, …same pitch draw…)
```

`Level.playSound(Entity, BlockPos, …)` plays at the block's centre (Level.txt 685–703).
`block.shulker_box.open` (1469), `block.shulker_box.close` (1468). No recheck: the
shulker box trusts its menus' `removed`.

**The block event on both sides** (`triggerEvent`, 301–326):

```
triggerEvent(1, n): openCount = n
                    if n == 0: animationStatus = CLOSING
                    if n == 1: animationStatus = OPENING
                    return true
otherwise: super.triggerEvent
```

The server runs block events through `triggerEvent` too (ADR-0026 section 4; the
repository's `block_events.rs`), so the server's animation starts in the block events'
phase of the tick after the click (the click is handled between ticks) and the ticker
moves it in the block entities' phase of the same tick. A count above 1 changes
nothing; a second opener while closing leaves it closing until the count passes
through 0 and 1 again (**Inferred** from the code: the count only rises to 2).

**The ticker** (`tick` → `updateAnimation`, 85–184), every tick on the server:

```
progressOld = progress
switch animationStatus:
    CLOSED:  progress = 0.0f
    OPENING: progress += 0.1f
             if progressOld == 0.0f: doNeighborUpdates(level, pos, state)
             if progress >= 1.0f: animationStatus = OPENED; progress = 1.0f; doNeighborUpdates(level, pos, state)
             moveCollidedEntities(level, pos, state)
    OPENED:  progress = 1.0f
    CLOSING: progress -= 0.1f
             if progressOld == 1.0f: doNeighborUpdates(level, pos, state)
             if progress <= 0.0f: animationStatus = CLOSED; progress = 0.0f; doNeighborUpdates(level, pos, state)
doNeighborUpdates (328–340): state.updateNeighbourShapes(level, pos, 3);
level.updateNeighborsAt(pos, state.getBlock())
```

`float` arithmetic (worked in `f32`): ten steps of `+ 0.1f` from 0 give `1.0000001f`,
so the tenth `OPENING` tick ends the opening; ten steps of `- 0.1f` from 1 give
`-7.45e-8f`, so the tenth `CLOSING` tick ends the closing. The box is fully open from
the tenth tick after the block event to the tenth after the last close's. The ticker
needs a typed block entity with
an in-memory animation (as the bell's), started by the block event.

**Pushing entities out** (`moveCollidedEntities`, 215–292), while `OPENING`, after the
progress step:

```
if state's block is not a ShulkerBoxBlock: return
dir = state.facing
box = Shulker.getProgressDeltaAabb(1.0f, dir, progressOld, progress,
Vec3.atBottomCenterOf(pos))
for e in level.getEntities((Entity) null, box):            // getEntities(Entity, AABB):
every entity, no predicate
    if e.getPistonPushReaction() == IGNORE_ENTITY: continue
    e.move(MoverType.SHULKER_BOX, new Vec3((box.getXsize() + 0.01) * dir.getStepX(),
                                           (box.getYsize() + 0.01) * dir.getStepY(),
                                           (box.getZsize() + 0.01) * dir.getStepZ()))
```

`MoverType.SHULKER_BOX` is a plain move for `Entity.move` (only `PISTON` is limited,
Entity.txt 1546–1569). A player is moved by its client, which runs the same ticker
(**Client**; the server's move of a player is overwritten by the player's next
position, as N4's pistons found).

**Shapes from the block entity**: `getShape` (268–286) is `Shapes.create(be
.getBoundingBox(state))` where `getBoundingBox` (192–213) is `Shulker.getProgressAabb(1.0f,
facing, 0.5f * getProgress(1.0f), Vec3(0.5, 0, 0.5))` (the block grows by up to half a
block towards `facing`), and `Shapes.block()` without a block entity; the collision
shape follows it (**Inferred**: `getCollisionShape` defaults to `getShape`).
`getBlockSupportShape` (241–266) is the full block when closed and, while not closed,
`SHAPES_OPEN_SUPPORT.get(facing.getOpposite())` (a 16 × 16 × 1 slab against the face
opposite the lid, **Inferred** from `Block.boxZ(16, 0, 1)` rotated): what is attached to
the box's sides loses support when it opens (`doNeighborUpdates`). `propagatesSkylightDown`
is false.

**Faces** (for hoppers): every face reaches all 27 slots (`getSlotsForFace`); nothing
that is a shulker box goes in (`canPlaceItemThroughFace`: `!(Block.byItem(item)
instanceof ShulkerBoxBlock)`, 532–542); anything comes out.

**Breaking** (`playerWillDestroy`, 138–206):

```
if be is ShulkerBoxBlockEntity:
    if server && player.preventsBlockDrops() && !be.isEmpty():          // a creative player
        stack = new ItemStack(state.getBlock()); stack.applyComponents(be.collectComponents())
        item = new ItemEntity(level, pos.x + 0.5, pos.y + 0.5, pos.z + 0.5, stack)   // its random motion: N4
        item.setDefaultPickUpDelay(); level.addFreshEntity(item)
    else:
        be.unpackLootTable(player)
super.playerWillDestroy(…)
```

`collectComponents` (BlockEntity.txt 729–743) is the block entity's kept components with
`custom_name`, `lock` (if locked), `container` (the 27 slots) and `container_loot` (if a
table is set) (BaseContainerBlockEntity.txt 272–299, RandomizableContainerBlockEntity.txt
163–182). Its loot table (`blocks/shulker_box.json`) has **no `survives_explosion`
condition** and copies `custom_name`, `container`, `lock` and `container_loot` from the
block entity; `getDrops` (208–231) also offers a dynamic drop `contents` (each slot) which
the table does not use. An empty box broken by a creative player drops nothing; in
survival the table drops the box with its contents. The shulker box's
`preRemoveSideEffects` (342–344) does nothing, so its contents never spill. Placing a
shulker box item gives the block entity its components back
(`applyImplicitComponents`, BaseContainerBlockEntity.txt 244–270: `custom_name`, `lock`,
`container` copied into the slots; `container_loot`). The dispenser's placing of
shulker boxes is the dispenser's section.

### 3.6 `Container` and the block entities' common parts

**`Container`** (Container.txt, 190 lines), defaults: `getMaxStackSize()` 99;
`getMaxStackSize(stack)` = `min(getMaxStackSize(), stack.getMaxStackSize())`;
`startOpen`/`stopOpen` nothing; `getEntitiesWithContainerOpen` empty; `canPlaceItem`
and `canTakeItem` true; `countItem`, `hasAnyOf`, `hasAnyMatching` over the slots in order.
`stillValidBlockEntity(be, player)` (116–148) with the buffer `4.0f`:

```
level = be.getLevel(); pos = be.getBlockPos()
if level == null: false
if level.getBlockEntity(pos) != be: false               // the block was replaced or its
chunk unloaded
player.isWithinBlockInteractionRange(pos, 4.0)          // Player.txt 4883–4905:
    // new AABB(pos).distanceToSqr(player.getEyePosition()) < (blockInteractionRange() + 4.0)^2
```

So a menu over a block stays valid while the player's eyes are within 8.5 (9 in
creative) of the block's box. Who asks and when (every tick, closing the menu) is 3.10
on.

**`CompoundContainer`** (CompoundContainer.txt, 245 lines): size the sum; slot `i < size1`
is `container1`'s `i`, else `container2`'s `i − size1`, for `getItem`, `removeItem`,
`removeItemNoUpdate`, `setItem`, `canPlaceItem`; `isEmpty` both; `getMaxStackSize` the
first's; `setChanged`, `startOpen`, `stopOpen`, `clearContent` the first, then the
second; `stillValid` both; `contains(c)` is identity with either.

**`SimpleContainer`** (463 lines) is the menus' stand-in on the client (each menu's
client constructor makes one) and the container of entities N7 brings; no block of
this section uses it on the server.

**`BaseContainerBlockEntity`** (BaseContainerBlockEntity.txt, 343 lines), over the kind's
`getItems()` list:

```
isEmpty: every slot empty
getItem(i): items[i]
removeItem(i, n): r = ContainerHelper.removeItem(items, i, n)   // items[i].split(n),
EMPTY when i out of range, empty or n <= 0
                  if !r.isEmpty(): setChanged()
removeItemNoUpdate(i): ContainerHelper.takeItem(items, i)       // items[i], replaced by
EMPTY; no setChanged
setItem(i, s): items[i] = s; s.limitSize(getMaxStackSize(s)); setChanged()
stillValid(player): Container.stillValidBlockEntity(this, player)
clearContent: items.clear()                                       // no setChanged
getName: custom name or getDefaultName(); getDisplayName = getName
```

`RandomizableContainerBlockEntity` (lines 45–94) makes `isEmpty`, `getItem`,
`removeItem`, `removeItemNoUpdate` and `setItem` unpack the loot table first
(`unpackLootTable(null)`); so does anything that reads a slot (a comparator, a hopper,
breaking). `unpackLootTable(player)` belongs to the `RandomizableContainer` interface,
not read; **Inferred** from the game's source:

```
if lootTable != null && level is server:
    table = server.reloadableRegistries().getLootTable(lootTable)
    (a ServerPlayer: the generate-loot criterion)
    lootTable = null; setChanged()
    params = LootParams(level).withParameter(ORIGIN, Vec3.atCenterOf(pos))
    if player != null: params.withLuck(player.getLuck()).withParameter(THIS_ENTITY, player)
    table.fill(this, params.create(LootContextParamSets.CHEST), lootTableSeed)
```

(The same shape as `ContainerEntity.unpackChestVehicleLootTable`, read: ContainerEntity
.txt 125–181, with the entity's position as the origin.) `LootTable.fill` (not read) takes
its random from the seed (**Inferred**: the level's random when the seed is 0, else a
fresh random of that seed) and spreads the rolled stacks over the empty slots at random
(the shuffle and splitting draw from that random).

**`setChanged`** (BlockEntity.txt 484–511): `level.blockEntityChanged(pos)` (the chunk
is unsaved, Level.txt 1411–1421) and, unless the block is air,
`level.updateNeighbourForOutputSignal(pos, block)` (Level.txt 1719 on, built in R4): the
comparators beside the container, and beyond a conductor, re-read it. Every change of
a slot by a menu, a hopper, a dispenser or a player goes through `setItem` or
`removeItem` (or the menu's slot's `setChanged`), so through this.

**Dropping contents** (`Containers`, Containers.txt; the N5 brief's 3.18 for carts):
`dropContents(level, pos, container)` drops each slot in order at `(x, y, z)` = the
block's corner; `dropItemStack(level, x, y, z, stack)` (67–151):

```
w = EntityTypes.ITEM.getWidth()  (float, widened: 0.25)
half = 1.0 - w; off = w / 2.0
x' = Math.floor(x) + random.nextDouble() * half + off        // three draws, x then y then z
y' = Math.floor(y) + random.nextDouble() * half
z' = Math.floor(z) + random.nextDouble() * half + off
while !stack.isEmpty():
    item = new ItemEntity(level, x', y', z', stack.split(random.nextInt(21) + 10))
    item.setDeltaMovement(random.triangle(0.0, 0.11485000171139836),
                          random.triangle(0.2, 0.11485000171139836),
                          random.triangle(0.0, 0.11485000171139836))
    level.addFreshEntity(item)
```

(the `y` term has no `off`, lines 92–100; the random is the level's; `ItemEntity`'s
constructor here is the five-argument one, which draws its own motion that the
`setDeltaMovement` then replaces, **Inferred** from N4's reading of `ItemEntity`). The
repository has this already for container vehicles (`region/vehicles.rs`,
`drop_contents`). `updateNeighboursAfterDestroy(state, level, pos)` (153–160) is
`level.updateNeighbourForOutputSignal(pos, state.getBlock())`, called by each of these
blocks' `affectNeighborsAfterRemoval` (ChestBlock.txt 372–378, BarrelBlock.txt 61–67,
ShulkerBoxBlock.txt 233–239; the others likewise, **Inferred**).

### 3.7 What is saved, and what the client is sent

Each block entity's data, as the game saves it (`saveWithoutMetadata` adds
`components`, BlockEntity.txt 273–284):

| Key | Written by | When |
|---|---|---|
| `lock` | `LockCode.addToTag` (BaseContainerBlockEntity.txt 35–50) | **Inferred** when locked (`LockCode` not read; the key from `removeComponentsFromTag`, 301–312) |
| `CustomName` | `storeNullable("CustomName", ComponentSerialization.CODEC, name)` | when named |
| `LootTable` | `trySaveLootTable` (**Inferred**, the interface not read) | while a table is set |
| `LootTableSeed` (long) | `trySaveLootTable` (**Inferred**) | while a table is set and the seed is not 0 |
| `Items` | `ContainerHelper.saveAllItems` (ContainerHelper.txt 54–100): `ItemStackWithSlot` entries (`Slot` byte and the stack), non-empty slots in order | when no table is set; chests and barrels always (`alwaysPutTag`), so an empty `Items: []`; shulker boxes pass `false`, so no `Items` when empty |
| `components` | `BlockEntity.saveWithoutMetadata` | always (the kept components, usually `{}`) |

Loading (`loadAdditional`): the list is made anew at the kind's size; with `LootTable`
present no items are read (`tryLoadLootTable` answers true); else each `Items` entry
whose slot is within the size is set (ContainerHelper.txt 102–130). Chunks keep this
data opaque today (`crates/clustine-world/src/chunk.rs`, `BlockEntity { kind, data }`,
at most `MAX_BLOCK_ENTITY_DATA` = 16 KiB each and 1 MiB a chunk); a full chest of
enchanted books can come near the first (**Inferred**; to measure). The hopper adds
`TransferCooldown`, the crafter its own fields, the dispenser nothing: their sections.

**To the client**: nothing of a container's state in `block_entity_data` (finding 14);
the chunk packet's entry of each block entity carries its type and an empty tag; the
lid of chests by block event (`1`, count) and of shulker boxes likewise; the barrel's
lid by its block state; the contents, the name (as the title) and the lock's refusal
only through the menu and the overlay message.

### 3.8 The comparator reading containers

`ComparatorBlock.getInputSignal` (ComparatorBlock.txt 151–230) is built (R4) and reads a
block's analog output where the block `hasAnalogOutputSignal`, behind it or beyond a
conductor (with an item frame's beyond, built in N5). The container blocks all answer
`hasAnalogOutputSignal` true and give:

| Block | `getAnalogOutputSignal` |
|---|---|
| chest family | `getRedstoneSignalFromContainer(getContainer(this, state, level, pos, false))` (ChestBlock.txt 597–611): the double chest's 54 slots together; **0 when blocked** (finding 5) |
| barrel, shulker box, dispenser, dropper, hopper | `getRedstoneSignalFromBlockEntity(level.getBlockEntity(pos))` (BarrelBlock.txt 94–105, ShulkerBoxBlock.txt 293–304, DispenserBlock.txt 349–360, HopperBlock.txt 261–272) |
| crafter | `CrafterBlockEntity.getRedstoneSignal()` (CrafterBlock.txt 44–65): the crafter's section |

```
getRedstoneSignalFromBlockEntity(be) (AbstractContainerMenu.txt 2223–2235):
    be is a Container ? getRedstoneSignalFromContainer(be) : 0
getRedstoneSignalFromContainer(c) (2237–2281):
    if c == null: 0
    f = 0.0f
    for i in 0 until c.getContainerSize():
        s = c.getItem(i)                                   // unpacks a loot table
        if !s.isEmpty(): f += (float) s.getCount() / (float) c.getMaxStackSize(s)
    f /= (float) c.getContainerSize()
    Mth.lerpDiscrete(f, 0, 15) = 0 + Mth.floor(f * (float)(15 - 0 - 1)) + (f > 0.0f ? 1 : 0)   // Mth.txt 1475–1498
```

All `float`; `getMaxStackSize(s)` is `min(99, s's max stack size)` for every container
here (none overrides `getMaxStackSize()`). The comparator is told by
`updateNeighbourForOutputSignal` on every `setChanged` and on removal (3.6). The
repository's `analog_output_at` (`region/rails.rs`, 326–334) is where a container's
reading goes, beside the detector rail's.

### 3.9 Container vehicles, and what exists to extend

**Chest and hopper carts, chest boats and chest rafts** exist since N5 and open nothing
(the N5 brief's 3.18; `ContainerEntity.interactWithContainerVehicle`, lines 116–123:
`player.openMenu(this)`; `SUCCESS`). To open them N6 needs, beside the menu: the
entity's display name as the title (its custom name or its type's name, **Inferred**:
`Entity.getDisplayName`), `createMenu` (a chest cart and a chest boat `ChestMenu
.threeRows`, a hopper cart `HopperMenu`; `null` for a spectator while a loot table is set,
AbstractChestBoat.txt 214–245), `stillValid` = `isChestVehicleStillValid` (not removed,
and within the entity interaction range + 4.0 of its box, ContainerEntity.txt 300–314),
the `CONTAINER_OPEN` game event at the entity on opening (`MinecartChest.interact`,
`AbstractChestBoat.interact`) and `CONTAINER_CLOSE` at its position on `stopOpen`
(MinecartChest.txt 52–64, AbstractChestBoat.txt 290–300). A vehicle has no openers'
count, no sound and no lid. A cart that moves out of range closes its menus (3.10 on);
one that crosses into another region does too (section 11).

**What the repository has, and what a porter extends:**

- `crates/clustine-sim/src/placement.rs`: `is_used`/`use_block` take the click on these
  blocks and change nothing (`Clicked::Nothing`); a new outcome opens a menu for the
  player, in the region that holds the block (section 11 says which).
- `crates/clustine-sim/src/region/block_entities.rs`: `Typed` has the bell, the
  comparator and the moving piston. The containers need a typed block entity that
  keeps their slots (and the lock, the name, the loot table and seed), read from and
  written back to the chunk's data in the game's form (3.7), as the comparator's
  output is; the openers' count in memory; and, for the shulker box, the animation with
  a ticker, as the bell's ring.
- `crates/clustine-sim/src/region/block_events.rs`: the chest's and the shulker box's
  event 1 joins the note block's and the piston's; the shulker box's `triggerEvent` has
  to run in the simulation (it starts the animation).
- `crates/clustine-sim/src/region/rails.rs` `analog_output_at`: containers' readings.
- `crates/clustine-sim/src/region/vehicles.rs` `drop_contents`: `Containers
  .dropContents`, to reuse for a block's removal (`preRemoveSideEffects`).
- `crates/clustine-world/src/chunk.rs`: block entities' data stays the store's form; a
  container's change is a change of that data in the commit (ADR-0026 section 6 lets a
  commit carry block entities).
- `services/edge/src/play.rs` and `fanout.rs`: no menu is opened anywhere; the edge
  needs `open_screen`, the menu's contents and slots, the close in both directions and
  the click packet (3.10 on), and the overlay message for locks.
- Loot: container loot tables (`chests/…`, `barrels/…`, `dispensers/…`) and
  `LootTable.fill` are new to `clustine-data` and the simulation.

### 3.10 Opening a menu: `ServerPlayer.openMenu(MenuProvider)` (ServerPlayer.txt 3191–3248)

The blocks of 3.1–3.9 call `player.openMenu(provider)` from `useWithoutItem` (the block
entity, or for a double chest the `MenuProvider` of `ChestBlock`, is the provider).

```
openMenu(provider):
  if provider == null: return empty
  if containerMenu != inventoryMenu: closeContainer()          // 3.18: sends container_close
  nextContainerCounter()                                        // counter = counter % 100 + 1
  menu = provider.createMenu(counter, getInventory(), this)
  if menu == null:
    if isSpectator(): sendOverlayMessage(translatable("container.spectatorCantOpen") RED)
    return empty
  send ClientboundOpenScreenPacket(menu.containerId, menu.getType(), provider.getDisplayName())
  initMenu(menu)                                                // 858–868
  containerMenu = menu
  return OptionalInt.of(counter)
```

- `containerCounter` starts at 0 (a field never set before), so the first menu is 1, the
  hundredth 100, the next 1 again. A menu's construction calls `container.startOpen(player)`
  (the openers' count and the lid, 3.1–3.9) before the open screen packet is sent, so the
  lid's block event can precede `open_screen` in the tick's packets (**Inferred** order on
  the wire: the block event goes out with the chunk's broadcasts at the end of the tick,
  the screen at once).
- `initMenu(menu)` (858–868): `menu.addSlotListener(containerListener)` (which calls
  `broadcastChanges()` at once, 336–352, with no synchroniser yet, so it only fills
  `lastSlots` and tells the listener: advancements, `ServerPlayer$2`, not read), then
  `menu.setSynchronizer(containerSynchronizer)` (353–369): the remote carried slot and every
  remote slot become `synchronizer.createSlot()`, and `sendAllDataToRemote()` (371–452):
  - every slot's stack is copied into a list and forced into its remote slot;
  - the carried stack is forced into the remote carried slot;
  - every data slot's value is copied into `remoteDataSlots`;
  - `synchronizer.sendInitialData(menu, items, carried.copy(), remoteDataSlots)`.
- What `sendInitialData` sends (**Inferred**, `ServerPlayer$1` not read; as 1.21's):
  `ClientboundContainerSetContentPacket(containerId, menu.incrementStateId(), items,
  carried)`, then for each data slot `i` `ClientboundContainerSetDataPacket(containerId, i,
  value)`.
- `initInventoryMenu()` (870–876) does the same for `inventoryMenu` (container id 0) at
  join: the content the edge sends at join today (`fanout.rs`, `SetContainerContent` with
  window 0, state id 0) is that packet; the game's state id there is 1 (the first
  `incrementStateId`, **Inferred**). The client takes any state id; the edge's 0 is fine as
  long as later packets count from it.

### 3.11 The menus and their slots

Each menu's constructor calls `checkContainerSize(container, n)` (throws when the container
is smaller), stores the container, calls `container.startOpen(inventory.player)` and adds
its slots; `removed(player)` calls `super.removed` (3.18) and then
`container.stopOpen(player)`. The player's own slots come last in every N6 menu, added by
`addStandardInventorySlots(inventory, x, y)` (AbstractContainerMenu.txt 172–190): first the
27 of the main inventory (`addInventoryExtendedSlots`, 130–170: inventory slot `9 + col +
row*9` for rows 0–2, columns 0–8), then the hotbar (`addInventoryHotbarSlots`, 105–128:
inventory slots 0–8). `addSlot` sets the slot's `index` to the menu's slot count and adds
an `EMPTY` to `lastSlots` and a remote slot (271–301).

| Menu (`MenuType`, wire id) | Container slots | Player's main 27 | Hotbar 9 | Other |
|---|---|---|---|---|
| `ChestMenu` 9×1…9×6 (`generic_9x1`…`generic_9x6`, 0–5) | 0 … 9r−1 (`addChestGrid`: container slot `col + row*9`) | 9r … 9r+26 | 9r+27 … 9r+35 | total 9r+36 |
| `DispenserMenu` (`generic_3x3`, 6) | 0–8 (`add3x3GridSlots`: `col + row*3`) | 9–35 | 36–44 | total 45 |
| `CrafterMenu` (`crafter_3x3`, 7) | 0–8 `CrafterSlot`s | 9–35 | 36–44 | 45 `NonInteractiveResultSlot` of a `ResultContainer`; 10 data slots |
| `HopperMenu` (`hopper`, 16) | 0–4 | 5–31 | 32–40 | total 41 |
| `ShulkerBoxMenu` (`shulker_box`, 20) | 0–26 `ShulkerBoxSlot`s | 27–53 | 54–62 | total 63 |

Wire ids are from `generated/reports/registries.json` (`minecraft:menu`).

- **`ChestMenu`** (ChestMenu.txt): the factories `oneRow` … `sixRows(id, inventory)` make a
  menu over a fresh `SimpleContainer(9*rows)` (the client's side); the server uses
  `threeRows(id, inventory, container)` (89–99) and `sixRows(id, inventory, container)`
  (101–111), so a chest, a trapped chest, a barrel, a chest minecart and a chest boat open
  `generic_9x3`, a double chest `generic_9x6` over a `CompoundContainer` (3.1–3.9);
  `generic_9x1`, `9x2`, `9x4` and `9x5` are not opened by anything in N6. The constructor
  (113–155) checks `rows*9`, calls `startOpen`, adds the grid at (8, 18) and the inventory
  at (8, 18 + rows*18 + 13). `stillValid` (198–204) is `container.stillValid(player)`.
  `getRowCount()` (288–292).
- **`DispenserMenu`** (DispenserMenu.txt 27–53): over the dispenser's or dropper's 9 slots;
  grid at (62, 17), inventory at (8, 84). Constants `INV_SLOT_START 9`, `INV_SLOT_END 36`,
  `USE_ROW_SLOT_START 36`, `USE_ROW_SLOT_END 45` (3–11).
- **`HopperMenu`** (HopperMenu.txt 19–61): over the hopper's (or hopper minecart's) 5 slots
  at (44 + i*18, 20); inventory at (8, 51).
- **`ShulkerBoxMenu`** (ShulkerBoxMenu.txt 19–80): 27 `ShulkerBoxSlot`s at (8 + col*18,
  18 + row*18); inventory at (8, 84). `ShulkerBoxSlot` is not dumped: its `mayPlace` refuses
  what cannot go inside a container item (**Inferred**:
  `stack.getItem().canFitInsideContainerItems()`,
  false for shulker boxes and bundles with contents, as in 1.21). Clicks, drags, swaps and
  quick moves all ask `mayPlace` before putting into an empty slot; merging into a slot that
  already holds the same item asks only `isSameItemSameComponents` (3.14.3), which cannot
  happen for a box in a box.
- **`CrafterMenu`** (CrafterMenu.txt 55–155): over the crafter's `CraftingContainer`
  (9 slots); `startOpen` on it; `CrafterSlot`s at (26 + col*18, 17 + row*18); inventory at
  (8, 84); then the result slot 45 at (134, 35) over a `ResultContainer`; then
  `addDataSlots(containerData)`: 10 data slots, 0–8 a slot's disabled flag (1 disabled),
  9 the triggered (powered) flag (`isSlotDisabled` 178–197, `isPowered` 199–210). The menu
  is its own listener (`addSlotListener(this)`, 88): every slot change runs
  `refreshRecipeResult` (285–318: on the server, `CrafterBlock.getPotentialResults(level,
  container.asCraftInput())` mapped to `recipe.assemble(input)`, or `EMPTY`, into the
  result container), so the client sees what the crafter would make. `setSlotState(slot,
  enabled)` (157–176) sets data `slot.index` to `enabled ? 0 : 1` and `broadcastChanges`.
  **The crafter menu has no `removed` override**: `stopOpen` is never called on its
  container (CrafterBlockEntity keeps no openers' count; section 7). `stillValid` (277–283)
  is the crafting container's.

### 3.12 `Slot` (Slot.txt, 343 lines)

A slot is a view of one index of a container. Defaults, which N6's plain slots keep:

| Method | Lines | Default |
|---|---|---|
| `mayPlace(stack)` | 65–68 | true |
| `mayPickup(player)` | 156–159 | true |
| `getItem()` | 70–77 | `container.getItem(slot)` |
| `hasItem()` | 79–88 | `!getItem().isEmpty()` |
| `set(stack)` | 106–116 | `container.setItem(slot, stack)`; `setChanged()` |
| `setByPlayer(stack)`, `setByPlayer(stack, old)` | 90–104 | `set(stack)` |
| `setChanged()` | 118–123 | `container.setChanged()` |
| `getMaxStackSize()` | 125–130 | `container.getMaxStackSize()` (99 by default, Container.txt 17–20) |
| `getMaxStackSize(stack)` | 132–139 | `min(getMaxStackSize(), stack.getMaxStackSize())` |
| `remove(n)` | 146–154 | `container.removeItem(slot, n)` |
| `onTake(player, stack)` | 59–63 | `setChanged()` |
| `onQuickCraft`, `onSwapCraft`, `checkTakeAchievements` | 31–57 | nothing (result slots override) |
| `allowModification(player)` | 304–318 | `mayPickup(player) && mayPlace(getItem())` |
| `safeClone(player)` | 229–238 | `getItem().copyWithCount(getItem().getMaxStackSize())` |

```
tryRemove(count, limit, player) (166–208):
  if !mayPickup(player): return empty
  if !allowModification(player) && limit < getItem().count: return empty
  count = min(count, limit)
  removed = remove(count)
  if removed.isEmpty(): return empty
  if getItem().isEmpty(): setByPlayer(EMPTY, removed)
  return Optional.of(removed)

safeTake(count, limit, player) (210–227):
  r = tryRemove(count, limit, player); r.ifPresent(s -> onTake(player, s)); return r or EMPTY

safeInsert(stack) (240–247) = safeInsert(stack, stack.count)
safeInsert(stack, n) (249–302):
  if stack.isEmpty() || !mayPlace(stack): return stack
  here = getItem()
  m = min(min(n, stack.count), getMaxStackSize(stack) - here.count)
  if m <= 0: return stack
  if here.isEmpty(): setByPlayer(stack.split(m))
  else if isSameItemSameComponents(here, stack): stack.shrink(m); here.grow(m); setByPlayer(here)
  return stack                                         // what is left of it
```

`ItemStack` helpers used (n5-javap ItemStack.txt): `isEmpty` (342–358: the `EMPTY`
instance, air, or a count ≤ 0); `split(n)` (375–390: takes `min(n, count)` as a copy and
shrinks this); `isStackable` (623–638: max stack size above 1 and not a damaged damageable
item); `matches(a, b)` (1153–1170: same instance, or same count and
`isSameItemSameComponents`); `isSameItem` (1204–1210: the item only);
`isSameItemSameComponents` (1212–1234: same item, and both empty or equal component maps).

### 3.13 `AbstractContainerMenu`'s state (AbstractContainerMenu.txt 1–86)

- `slots` (the slots in order), `lastSlots` (the stacks the listeners were last told),
  `remoteSlots` and `remoteCarried` (what the client is believed to show: `RemoteSlot`s,
  `PLACEHOLDER` until a synchroniser is set), `dataSlots` and `remoteDataSlots` (ints);
- `carried` (the stack on the cursor, `EMPTY` at first), `stateId` (0 at first),
  `containerId`, `menuType`;
- `quickcraftType` (−1 at first), `quickcraftStatus` (0), `quickcraftSlots` (a `HashSet`
  of slots);
- `containerListeners`, `synchronizer`, `suppressRemoteUpdates`.

Constants (3–27): `SLOT_CLICKED_OUTSIDE = -999`, buttons `CONTAINER_CLICK_PRIMARY 0` and
`SECONDARY 1`, quick craft types `CHARITABLE 0`, `GREEDY 1`, `CLONE 2`, headers `START 0`,
`CONTINUE 1`, `END 2`, `CARRIED_SLOT_SIZE = Integer.MAX_VALUE`.

None of this is saved: a menu lives while it is open. The carried stack is lost if the
server stops with a menu open (the game does not save it; at an orderly logout `removed`
puts it back, 3.18).

### 3.14 `clicked` and `doClick` (847–1723)

`clicked(slot, button, input, player)` (847–910) calls `doClick` and turns any exception
into a crash report ("Container click": menu type, class, slot count, slot, button, type).

`doClick` (912–1723), in order (`inventory` is `player.getInventory()`):

#### 3.14.1 Quick craft (drag), `input == QUICK_CRAFT` (917–1146)

The button encodes `header = button & 3` (`getQuickcraftHeader`, 2102–2107) and `type =
(button >> 2) & 3` (`getQuickcraftType`, 2093–2100); the client builds it as
`getQuickcraftMask(header, type) = (header & 3) | ((type & 3) << 2)` (2109–2120). So a left
drag sends 0, 1 (per slot), 2; a right drag 4, 5, 6; a middle drag 8, 9, 10; the start and
end come with slot −999 (**Client**).

```
prev = quickcraftStatus
quickcraftStatus = header
if !(prev == 1 && quickcraftStatus == 2) && prev != quickcraftStatus: resetQuickCraft();
return
if getCarried().isEmpty(): resetQuickCraft(); return
if quickcraftStatus == 0:                                   // start
  quickcraftType = type
  if isValidQuickcraftType(type, player): quickcraftStatus = 1; quickcraftSlots.clear()
  else resetQuickCraft()
elif quickcraftStatus == 1:                                 // a slot dragged over
  s = slots[slot]; c = getCarried()
  if canItemQuickReplace(s, c, true) && s.mayPlace(c)
     && (quickcraftType == 2 || c.count > quickcraftSlots.size()) && canDragTo(s):
    quickcraftSlots.add(s)
elif quickcraftStatus == 2:                                 // end
  if !quickcraftSlots.isEmpty():
    if quickcraftSlots.size() == 1:
      i = the one slot's index; resetQuickCraft()
      doClick(i, quickcraftType, PICKUP, player); return    // a drag over one slot is a click
    copy = getCarried().copy()
    if copy.isEmpty(): resetQuickCraft(); return
    left = getCarried().count
    for s in quickcraftSlots:                               // HashSet order
      c = getCarried()                                      // unchanged during the loop
      if s != null && canItemQuickReplace(s, c, true) && s.mayPlace(c)
         && (quickcraftType == 2 || c.count >= quickcraftSlots.size()) && canDragTo(s):
        j = s.hasItem() ? s.getItem().count : 0
        k = min(copy.getMaxStackSize(), s.getMaxStackSize(copy))
        l = min(getQuickCraftPlaceCount(quickcraftSlots.size(), quickcraftType, copy) + j, k)
        left -= l - j
        s.setByPlayer(copy.copyWithCount(l))
    copy.setCount(left); setCarried(copy)
  resetQuickCraft()
else: resetQuickCraft()
```

- Note the start's comparison: a start (`header 0`) while `status` is 0 passes (`prev ==
  new`), then sets `status = 1`; continues (`1`, `1`) pass; the end (`1` then `2`) passes;
  anything else (a second start in the middle, an end without a start) resets and does
  nothing.
- `isValidQuickcraftType(type, player)` (2122–2142): 0 and 1 always; 2 only if
  `player.hasInfiniteMaterials()` (creative).
- `getQuickCraftPlaceCount(n, type, stack)` (2192–2216): type 0 `Mth.floor((float) count /
  (float) n)` (a `float` division, then `Mth.floor(float)`); 1: 1; 2:
  `stack.getMaxStackSize()`; otherwise `count`.
- `canItemQuickReplace(slot, stack, true)` (2154–2190): true if the slot is null or empty;
  else `isSameItemSameComponents(stack, slot item) && slot.count + 0 <=
  stack.getMaxStackSize()`
  (with `false` the stack's count is added).
- `canDragTo(slot)` (2218–2221): true (result slots' menus override; none of N6's do,
  **Inferred** for `CrafterMenu`, which does not override it in its dump).
- `resetQuickCraft()` (2144–2152): `status = 0`, slots cleared (the type is kept).
- The one-slot case re-enters as `PICKUP` with `button = quickcraftType`: 0 a left click, 1
  a right click, and 2 (a middle drag over one slot) is no click button at all, so it does
  nothing (3.14.3's `isContainerClickButton`).
- **The order of the `HashSet` does not matter**: each slot's `l` depends only on the slot,
  `copy` and the set's size; `left` is a sum. With type 2 (creative) `left` can fall to zero
  or below, and `setCount(left ≤ 0)` leaves the carried stack empty (a count ≤ 0 is empty).
- Any click of another kind while a drag is in progress (`quickcraftStatus != 0`) only
  resets the drag (1147–1152) and does nothing else.

#### 3.14.2 Outside the window: `PICKUP` or `QUICK_MOVE` with slot −999 (1153–1195)

Only for `PICKUP` or `QUICK_MOVE` with button 0 or 1 (`isContainerClickButton`, 1725–1735;
`getClickAction`, 1737–1755 throws for any other button but is called only after that
test). If the carried stack is not empty: button 0 throws it all
(`player.drop(getCarried(), true, PREDICTED)`; carried becomes `EMPTY`), button 1 throws one
(`player.drop(getCarried().split(1), true, PREDICTED)`).

#### 3.14.3 `PICKUP` with a slot, button 0 or 1 (1233–1375)

```
if slot < 0: return                        // -1 is a click in the window's margin
s = slots[slot]; here = s.getItem(); c = getCarried()
player.updateTutorialInventoryAction(c, here, action)        // nothing on a server
if !tryItemClickBehaviourOverride(player, action, s, here, c):
  if here.isEmpty():
    if !c.isEmpty():
      n = action == PRIMARY ? c.count : 1
      setCarried(s.safeInsert(c, n))
  elif s.mayPickup(player):
    if c.isEmpty():
      n = action == PRIMARY ? here.count : (here.count + 1) / 2
      s.tryRemove(n, Integer.MAX_VALUE, player).ifPresent(t -> { setCarried(t); s.onTake(player, t) })
    elif s.mayPlace(c):
      if isSameItemSameComponents(here, c):
        n = action == PRIMARY ? c.count : 1
        setCarried(s.safeInsert(c, n))
      elif c.count <= s.getMaxStackSize(c):
        setCarried(here); s.setByPlayer(c)                   // swap
    elif isSameItemSameComponents(here, c):                  // a slot that takes nothing (a result)
      s.tryRemove(here.count, c.getMaxStackSize() - c.count, player)
        .ifPresent(t -> { c.grow(t.count); s.onTake(player, t) })
s.setChanged()                                               // always
```

`tryItemClickBehaviourOverride` (1757–1791): if the carried stack's item is enabled and
`c.overrideStackedOnOther(s, action, player)` says so, or the slot's item is enabled and
`here.overrideOtherStackedOnMe(c, s, action, player, carriedAccess)` says so, the click is
the item's (bundles; `AbstractContainerMenu$1` is the carried slot's access: `get` and `set`
the carried stack). Not read (`ItemStack.overrideStackedOnOther` and the bundle); with no
bundles in N6 it can be treated as false.

#### 3.14.4 `QUICK_MOVE` (shift-click), button 0 or 1 (1196–1232)

```
if slot < 0: return
s = slots[slot]
if !s.mayPickup(player): return
r = quickMoveStack(player, slot)
while !r.isEmpty() && isSameItem(s.getItem(), r): r = quickMoveStack(player, slot)
```

`quickMoveStack` is the menu's (3.14.8). The loop moves again while the slot still holds the
same item (a result slot crafting again, or a stack that moved only in part until no room
is left, when `quickMoveStack` returns `EMPTY`).

`moveItemStackTo(stack, start, end, reverse)` (1940–2091):

```
moved = false
i = reverse ? end - 1 : start
if stack.isStackable():                                  // pass 1: merge
  while !stack.isEmpty() && (reverse ? i >= start : i < end):
    s = slots[i]; t = s.getItem()
    if !t.isEmpty() && isSameItemSameComponents(stack, t):
      total = t.count + stack.count; max = s.getMaxStackSize(t)
      if total <= max: stack.setCount(0); t.setCount(total); s.setChanged(); moved = true
      elif t.count < max: stack.shrink(max - t.count); t.setCount(max); s.setChanged(); moved = true
    i += reverse ? -1 : 1
if !stack.isEmpty():                                     // pass 2: the first empty slot
that takes it
  i = reverse ? end - 1 : start
  while reverse ? i >= start : i < end:
    s = slots[i]
    if s.getItem().isEmpty() && s.mayPlace(stack):
      max = s.getMaxStackSize(stack)
      s.setByPlayer(stack.split(min(stack.count, max))); s.setChanged(); moved = true
      break
    i += reverse ? -1 : 1
return moved
```

Pass 1 does not ask `mayPlace`; pass 2 stops at the first empty slot it fills.

#### 3.14.5 `SWAP` (a number key over a slot, or F), button 0–8 or 40 (1376–1506)

Only for `button` in 0..8 (the hotbar slot) or 40 (the off hand); any other button does
nothing. `slot` is not checked (a negative one throws, 3.0 item 25).

```
held = inventory.getItem(button); s = slots[slot]; here = s.getItem()
if held.isEmpty() && here.isEmpty(): nothing
elif held.isEmpty():
  if s.mayPickup(player):
    inventory.setItem(button, here); s.onSwapCraft(here.count); s.setByPlayer(EMPTY); s.onTake(player, here)
elif here.isEmpty():
  if s.mayPlace(held):
    max = s.getMaxStackSize(held)
    if held.count > max: s.setByPlayer(held.split(max))
    else: inventory.setItem(button, EMPTY); s.setByPlayer(held)
elif s.mayPickup(player) && s.mayPlace(held):
  max = s.getMaxStackSize(held)
  if held.count > max:
    s.setByPlayer(held.split(max)); s.onTake(player, here)
    if !inventory.add(here): player.drop(here, true, PREDICTED)
  else:
    inventory.setItem(button, here); s.setByPlayer(held); s.onTake(player, here)
```

The hotbar and off hand are written in the `Inventory` directly; when the slot clicked is
itself one of the player's (in the menu's lower part) the two are the same container.

#### 3.14.6 `CLONE` (middle click) (1507–1533)

Only if `player.hasInfiniteMaterials()`, the carried stack is empty and `slot >= 0`: if the
slot has an item, `setCarried(s.safeClone(player))` (a full stack of it). Otherwise nothing.

#### 3.14.7 `THROW` (Q over a slot), button 0 (one) or 1 (the stack) (1534–1607)

Only with an empty carried stack and `slot >= 0`:

```
s = slots[slot]
n = button == 0 ? 1 : s.getItem().count
if !player.canDropItems(): return                    // true for a player (Player.txt 3206)
t = s.safeTake(n, Integer.MAX_VALUE, player)
player.drop(t, true, PREDICTED); player.handleCreativeModeItemDrop(t)   // the latter
does nothing
if button == 1:
  while !t.isEmpty() && isSameItem(s.getItem(), t):   // a result slot that makes more
    if !player.canDropItems(): return
    t = s.safeTake(n, Integer.MAX_VALUE, player)
    player.drop(t, true, PREDICTED); player.handleCreativeModeItemDrop(t)
```

`drop` of an empty stack makes nothing (LivingEntity.drop returns null, n5 LivingEntity.txt
1657–1695). `Prediction.PREDICTED` makes the swing go to the other players but not to the
thrower (`swing(MAIN_HAND, DEFAULT, prediction != PREDICTED)`, same lines). The thrown
item is
the N2 throw (`createItemStackToDrop(stack, false, true)`: thrown from the eyes in the
look's
direction, with the thrower and its draws, section 10). Unlike the creative inventory's
throw
(`set_creative_mode_slot` with −1), this throw is **not** throttled.

#### 3.14.8 `PICKUP_ALL` (double click), button 0 or 1 (1608–1722)

```
if slot < 0: return
s = slots[slot]; c = getCarried()
if c.isEmpty() || (s.hasItem() && s.mayPickup(player)): return   // the first click
picked it up
start = button == 0 ? 0 : slots.size() - 1; step = button == 0 ? 1 : -1
for pass in 0..2:
  for i = start; 0 <= i < slots.size() && c.count < c.getMaxStackSize(); i += step:
    t = slots[i]
    if t.hasItem() && canItemQuickReplace(t, c, true) && t.mayPickup(player)
       && canTakeItemForPickAll(c, t):                             // true by default (1801–1804)
      here = t.getItem()
      if pass == 0 && here.count == here.getMaxStackSize(): continue   // full stacks last
      taken = t.safeTake(here.count, c.getMaxStackSize() - c.count, player)
      c.grow(taken.count)
```

So a double click gathers first from partial stacks, then from full ones, from the first
slot (left button) or the last (right), container and inventory alike, until the cursor is
full.

#### 3.14.9 The menus' `quickMoveStack(player, index)`

All read `s = slots[index]`; if it has an item, `stack = s.getItem()`, `copy =
stack.copy()`,
move, and then `stack.isEmpty() ? s.setByPlayer(EMPTY) : s.setChanged()` (the crafter uses
`s.set(EMPTY)`):

| Menu | From the container (index below n) | From the player | After | Lines |
|---|---|---|---|---|
| `ChestMenu` (n = 9r) | `moveItemStackTo(stack, 9r, size, true)` | `(0, 9r, false)` | returns `copy` | ChestMenu.txt 206–269 |
| `DispenserMenu` (n = 9) | `(9, 45, true)` | `(0, 9, false)` | `EMPTY` if `stack.count == copy.count`, else `s.onTake(player, stack)`, returns `copy` | DispenserMenu.txt 105–168 |
| `HopperMenu` (n = 5) | `(5, size, true)` | `(0, 5, false)` | returns `copy` | HopperMenu.txt 71–131 |
| `ShulkerBoxMenu` (n = 27) | `(27, size, true)` | `(0, 27, false)` | returns `copy` | ShulkerBoxMenu.txt 90–150 |
| `CrafterMenu` (n = 9) | `(9, 45, true)` | `(0, 9, false)` | as the dispenser's | CrafterMenu.txt 212–275 |

If the move returns false the method returns `EMPTY` at once (nothing changed). So a
shift-click from a chest goes to the hotbar's right end first and then up the inventory
(reverse), and from the inventory into the chest's first slots. The crafter's result slot 45
is index ≥ 9, so its quick move would try the grid; `NonInteractiveResultSlot.mayPickup` is
false (**Inferred**, not read), which stops `QUICK_MOVE` before it.

### 3.15 Synchronising: `broadcastChanges`, `broadcastFullState`, remote slots

```
broadcastChanges() (487–553):
  for i in slots:
    stack = slots[i].getItem(); copy = memoize(stack.copy)
    triggerSlotListeners(i, stack, copy)          // 630–669: if !matches(lastSlots[i], stack):
                                                  //   lastSlots[i] = copy; each listener.slotChanged(menu, i, copy)
    synchronizeSlotToRemote(i, stack, copy)       // 671–701
  synchronizeCarriedToRemote()                    // 734–761
  for i in dataSlots:
    v = dataSlots[i].get()
    if dataSlots[i].checkAndClearUpdateFlag(): updateDataSlotListeners(i, v)   // 609–628
    synchronizeDataSlotToRemote(i, v)             // 703–732

synchronizeSlotToRemote(i, stack, copy):
  if suppressRemoteUpdates: return
  if !remoteSlots[i].matches(stack):
    remoteSlots[i].force(stack); synchronizer?.sendSlotChange(menu, i, copy())
synchronizeCarriedToRemote():
  if suppressRemoteUpdates: return
  if !remoteCarried.matches(carried): remoteCarried.force(carried); synchronizer?.sendCarriedChange(menu, carried.copy())
synchronizeDataSlotToRemote(i, v):
  if suppressRemoteUpdates: return
  if remoteDataSlots[i] != v: remoteDataSlots[i] = v; synchronizer?.sendDataChange(menu, i, v)

broadcastFullState() (555–607):
  for i in slots: triggerSlotListeners(i, stack, stack::copy)
  for i in dataSlots: if checkAndClearUpdateFlag(): updateDataSlotListeners(i, get())
  sendAllDataToRemote()                           // 3.10: everything, forced
```

- `RemoteSlot` and `RemoteSlot$Synchronized` are not dumped. **Inferred** (as 1.21.5's): a
  synchronised remote slot holds either the last stack forced into it (`force(stack)`, a
  copy) or the last hash the client reported (`receive(hashed)`, which drops the stack);
  `matches(stack)` compares a held stack with `ItemStack.matches`, or a held hash with
  `HashedStack.matches(stack, generator)`; `copyFrom` copies both (used by
  `transferState`). `PLACEHOLDER` matches everything and forces nothing.
- What the synchroniser sends (**Inferred**, `ServerPlayer$1` not read):
  `sendSlotChange` → `ClientboundContainerSetSlotPacket(containerId,
  menu.incrementStateId(), slot, stack)`; `sendCarriedChange` →
  `ClientboundSetCursorItemPacket(stack)` (`set_cursor_item`, 98); `sendDataChange` →
  `ClientboundContainerSetDataPacket(containerId, id, value)`; `createSlot` → a
  `RemoteSlot.Synchronized` with the connection's hash generator.
- `incrementStateId()` (2443–2455): `stateId = (stateId + 1) & 32767`, returned. So it
  runs 1
  to 32767 and then 0. `setItem(slot, stateId, stack)` (1889–1899) and
  `initializeContents(stateId, items, carried)` (1901–1925) are the client's side.
- Listeners: `ServerPlayer`'s `containerListener` (`$2`, not read: advancements'
  `INVENTORY_CHANGED` on the player's own slots); `CrafterMenu` (3.11).
- **When `broadcastChanges` runs**: every `ServerPlayer.tick` for `containerMenu` (939–940);
  after every click (3.17); in `addSlotListener` (336–352); `slotsChanged(container)`
  (1883–1887,
  called by containers that notify their menu: `SimpleContainer`'s listeners, the crafting
  grid; **Inferred** that block entities' containers do not call it, their `setChanged` only
  marking the chunk); `ServerPlayer.take` (pick-up, 2716–2725); a successful
  `clickMenuButton` (SGPLI 6216–6223); `CrafterMenu.setSlotState`; `openItemGui`. So a
  hopper or a dispenser changing an open chest's contents reaches its viewers at the next
  player tick.

### 3.16 Hashed stacks (`HashedStack`, `HashedPatchMap`)

- `HashedStack` (HashedStack.txt): `EMPTY` (`HashedStack$1`, whose `matches(stack, g)` is
  `stack.isEmpty()`, 14–18) or `ActualItem(item, count, components)`
  (HashedStack$ActualItem.txt).
  `create(stack, g)` (9–27): `EMPTY` for an empty stack, else
  `ActualItem(stack.typeHolder(),
  stack.getCount(), HashedPatchMap.create(stack.getComponentsPatch(), g))`.
- `ActualItem.matches(stack, g)` (26–49): same count, same item holder, and
  `components.matches(stack.getComponentsPatch(), g)`.
- `HashedPatchMap(addedComponents: Map<type, Integer>, removedComponents: Set<type>)`
  (HashedPatchMap.txt). `create(patch, g)` (21–45): split the patch; for each added
  component
  `put(type, g.apply(typed component))`; the removed set as is. `matches(patch, g)`
  (47–104):
  the patch's removed set equals `removedComponents`; the added maps have the same size; and
  for every added component of the real stack the client's map has its type and the hash
  `g.apply(component)` equals the client's integer.
- `HashGenerator` (HashedPatchMap$HashGenerator.txt) is a `Function<TypedDataComponent,
  Integer>`; its implementation (the connection's, a CRC32C over the component encoded with
  `HashOps`, **Inferred** from SteelMC's 26.3 code) is not read.
- For Clustine's stacks without components the patch is empty: a client's hash matches a
  stack exactly when it is `EMPTY` and the stack is empty, or it is an item with the same id
  and count and both component maps empty (4.0 item 4).

### 3.17 `handleContainerClick` (ServerGamePacketListenerImpl.txt 5900–6052)

```
ensureRunningOnSameThread
player.resetLastActionTime()
if player.containerMenu.containerId != packet.containerId: return              // silently
if player.isSpectator() || player.isDeadOrDying(): containerMenu.sendAllDataToRemote();
return
if !containerMenu.stillValid(player): log; return                               // silently
slot = packet.slotNum
if !containerMenu.isValidSlotIndex(slot): log; return                           // 253–269
stale = packet.stateId != containerMenu.getStateId()
containerMenu.suppressRemoteUpdates()                                            //
2296–2301
containerMenu.clicked(slot, packet.buttonNum, packet.containerInput, player)
for (i, hashed) in packet.changedSlots: containerMenu.setRemoteSlotUnsafe(i, hashed)
  // 774–800: an index outside the remote slots is logged and skipped; else remoteSlots[i].receive(hashed)
containerMenu.setRemoteCarried(packet.carriedItem)                               // 802–808
containerMenu.resumeRemoteUpdates()                                              //
2303–2308
if stale: containerMenu.broadcastFullState() else containerMenu.broadcastChanges()
```

- The container id test also covers the player's own inventory (id 0) when no menu is open.
- `isValidSlotIndex(i)` (253–269): `i == -1 || i == -999 || i < slots.size()` — so any other
  negative index is "valid" too (`i < size` holds) and reaches `doClick`; every branch but
  `SWAP` and the drag's continue returns on a negative slot. The port: refuse negatives
  other
  than −999 and −1 for every kind, and −999/−1 for `SWAP` and the drag's continue (3.0
  item 11).
- `suppressRemoteUpdates` makes `synchronize*ToRemote` return at once, so nothing is sent
  during the click (listeners still run if the click triggers `broadcastChanges`).
- With a stale state id the client is sent the whole menu (`set_container_content` with a
  new
  state id, carried, and the data), whatever its prediction.

### 3.18 Closing (`handleContainerClose`, `closeContainer`, `doCloseContainer`, `removed`)

- `handleContainerClose` (SGPLI 5887–5898): `player.doCloseContainer()`. **The packet's
  container id is not compared**: any close closes the open menu (or, with none open, runs
  on `inventoryMenu`).
- `closeContainer()` (ServerPlayer.txt 3388–3401), the server's own closing: send
  `ClientboundContainerClosePacket(containerMenu.containerId)`, then `doCloseContainer()`.
  Called by `openMenu` (another menu), by `tick`/`doTick` when `stillValid` fails (939–951,
  1085–1094), and by others not in N6 (sleeping, dimension change, death: **Inferred**).
- `doCloseContainer()` (3403–3418): `containerMenu.removed(player)`;
  `inventoryMenu.transferState(containerMenu)`; `containerMenu = inventoryMenu`.
- `AbstractContainerMenu.removed(player)` (1806–1824): only for a `ServerPlayer`: if the
  carried stack is not empty, `dropOrPlaceInInventory(player, carried)` and carried =
  `EMPTY`. `dropOrPlaceInInventory` (1826–1864): if the player is removed (for any reason
  but `CHANGED_DIMENSION`) or has disconnected, `player.drop(stack, false, SERVER_ONLY)`;
  else `inventory.placeItemBackInInventory(stack, SERVER_ONLY)` (not read: **Inferred** as
  1.21's, the selected slot if it can take it, else the first slot that can, merging first,
  and what is left dropped).
- Then the menu's own `removed` calls `container.stopOpen(player)` (ChestMenu.txt 271–280,
  DispenserMenu.txt 170–179, HopperMenu.txt 133–142, ShulkerBoxMenu.txt 152–161): the
  openers'
  count and the lid (3.1–3.9). Not the crafter (3.11).
- `transferState(old)` (2310–2404): for every slot of the inventory menu whose
  `(container, containerSlot)` the old menu also showed, copy the old menu's `lastSlots`
  entry and (if both are `Synchronized`) its remote slot. The client was told the player's
  slots through the chest menu; the inventory menu now knows what it was told.
- `clearContainer(player, container)` (1866–1881): every slot of `container` removed and
  `dropOrPlaceInInventory`'d (used by crafting tables and the like; not by N6's menus).

### 3.19 The creative inventory: `handleSetCreativeModeSlot` (SGPLI 6226–6329)

```
if !player.hasInfiniteMaterials(): return
drop = packet.slotNum < 0
stack = packet.itemStack
if !stack.isItemEnabled(level.enabledFeatures()): return
inMenu = 1 <= slotNum && slotNum <= 45
valid = stack.isEmpty() || stack.count <= stack.getMaxStackSize()
if inMenu && valid:
  inventoryMenu.getSlot(slotNum).setByPlayer(stack)
  inventoryMenu.setRemoteSlot(slotNum, stack)          // the client has it already
  inventoryMenu.broadcastChanges()
elif drop && valid:
  if dropSpamThrottler.isUnderThreshold(): increment; player.drop(stack, true, PREDICTED)
  else log "dropping items too fast in creative mode, ignoring"
```

- The slots are the inventory menu's (`InventoryMenu`, not read): 0 the 2×2 result
  (refused: `slotNum >= 1`), 1–4 the 2×2 grid, 5–8 armour (head first), 9–35 the main
  inventory, 36–44 the hotbar, 45 the off hand. The edge maps 5–45 to the inventory today
  (`Inventory::from_container`) and ignores 1–4 (3.0 item 24).
- `setByPlayer` on an armour slot (`ArmorSlot`, not read) runs its equip logic
  (**Inferred**: `onEquipItem` and the equip sound, as a stand's equipment does); `mayPlace`
  is not asked.
- The creative screen sends this packet for its own inventory tab and for the hotbar while
  any creative tab is shown (**Client**); with a chest open, a creative player's clicks are
  `container_click`s like anyone's, with `CLONE` and type-2 drags allowed
  (`hasInfiniteMaterials`).
- The edge already throttles the throw (`DropThrottler` in `play.rs`, the game's
  `TickThrottler(20, 1480)`) and must check `valid` (count ≤ the item's max stack size).

### 3.20 Other packets of menus

- `handleContainerButtonClick` (SGPLI 6170–6224): container id equal and not a spectator,
  `stillValid`, then `clickMenuButton(player, buttonId)` and `broadcastChanges` if true.
  False for every N6 menu: nothing to do.
- `handleContainerSlotStateChanged` (SGPLI 2824–2869): not a spectator and container id
  equal;
  if the open menu is a `CrafterMenu` whose container is a `CrafterBlockEntity`:
  `setSlotState(packet.slotId, packet.newState)` (section 7).
- `handlePlaceRecipe` (6054–6168): the recipe book; only for `RecipeBookMenu`s, none of
  N6's.

### 3.21 The player's own inventory menu and what the edge and the region each keep

- `inventoryMenu` (container id 0, `InventoryMenu`, not dumped) is always there and is
  `containerMenu` when nothing else is open. Its 46 slots are as 3.19's list. A survival
  player's clicks in it are `container_click`s with id 0 (all of 3.14 applies, over the
  inventory and the 2×2 grid); a creative player's own inventory goes by
  `set_creative_mode_slot` (**Client**). N6's players are creative: clicks with id 0 can be
  refused for now (the client then resyncs on its next stale state id; **Inferred** that the
  creative client never sends them).
- **Proposal for the split** (ADR-0005; to be agreed in the plan):

| Kept by | What |
|---|---|
| The edge | the container id counter (1..100), the open menu's id and type, the state id, the remote slots (the stacks sent, or the hashes the client reported), the remote carried stack and data, the drop throttler; decoding `HashedStack`s and comparing them; every packet: `open_screen`, `container_set_content`, `container_set_slot`, `set_cursor_item`, `container_set_data`, `container_close` |
| The region | which container the open menu shows (a block position, or an entity key, and the kind), the carried stack, the drag's state (status, type, slots), every rule of 3.14, `stillValid`, the openers' count and the lid, what is dropped or put back at closing |

  The edge turns a `container_click` into an input (container id checked by the edge; slot,
  button, kind) and, after the region's answer, does
  `setRemoteSlotUnsafe`/`setRemoteCarried`
  with the packet's hashes and compares (3.17's order: the click first, the hashes after).
  The region answers each click and each tick with the open menu's slots and carried stack
  (or the slots that changed since it last told the edge, as `lastSlots` does for
  listeners), and with "closed" when it closes a menu (the edge then sends
  `container_close`).
  The state id stays the edge's; staleness is the edge's test, but the click still goes to
  the region.

## 4. Dispensers and droppers

Read: `DispenserBlock` (DispenserBlock.txt, 422 lines), `DropperBlock` (132),
`DispenserBlockEntity` (204), `DropperBlockEntity` (25), `BlockSource` (78), every class of
`net.minecraft.core.dispenser` (`DispenseItemBehavior` and its thirteen anonymous classes
`$1` to `$13`, `DefaultDispenseItemBehavior`, `OptionalDispenseItemBehavior`,
`BoatDispenseItemBehavior`, `MinecartDispenseItemBehavior`, `EquipmentDispenseItemBehavior`,
`FlintAndSteelDispenseItemBehavior`, `ShearsDispenseItemBehavior`,
`ShulkerBoxDispenseBehavior`, `SpawnEggItemBehavior`, `ProjectileDispenseBehavior`,
`SulfurCubeBlockDispenseItemBehavior`), `TntBlock.prime` and `onPlace`/`neighborChanged`
(TntBlock.txt 21–60, 131–221), the `PrimedTnt` constructor (PrimedTnt.txt 43–90),
`EntityType.spawn`/`create` with a processor (EntityType.txt 194–316),
`HopperBlockEntity.addItem`, `tryMoveInItem`, `getContainerAt`, `getBlockContainer`,
`getEntityContainer` (HopperBlockEntity.txt 588–982), `DispenserMenu` (its slots only),
and from the N5 dump `LivingEntity.canEquipWithDispenser`, `canDispenserEquipIntoSlot`,
`getEquipmentSlotForItem` (LivingEntity.txt 10680–10780), `Mob.canDispenserEquipIntoSlot`
(Mob.txt 2715–2719) and `ArmorStand.canUseSlot` (ArmorStand.txt 201–227). Level-event
names are quoted from SteelMC's extraction
(`steel-registry/build_assets/level_events.json`):
`LevelEvent` is not dumped.

### 4.0 Findings of this section

1. **A dispenser fires 4 ticks after a rising edge, whatever the power then.**
   `neighborChanged` (207–265) reads `hasNeighborSignal(pos) ||
   hasNeighborSignal(pos.above())`
   (quasi-connectivity, as the piston's); powered and not `triggered` → `scheduleTick(pos,
   block, 4)` and `triggered=true` with flags 2; unpowered and `triggered` →
   `triggered=false` with flags 2. The scheduled `tick` (266–274) only calls
   `dispenseFrom`: it does not look at the power again. There is no `onPlace`: a dispenser
   placed beside a powered block waits for a neighbour update. The dropper and the crafter
   inherit or copy this rule; the crafter reads only its own position (7.0 item 1).
2. **The slot is chosen by reservoir sampling over the non-empty slots, one draw each.**
   `getRandomSlot` (DispenserBlockEntity.txt 37–70): first `unpackLootTable(null)`; then
   `n = 1`, and for each slot `i` in 0..8 that is not empty: `if random.nextInt(n++) == 0`
   then `chosen = i`. The random is the **level's** (`ServerLevel.getRandom()`,
   DispenserBlock.txt 91–156 at 48–54). Every non-empty slot draws, the first included
   (`nextInt(1)`). An empty dispenser draws nothing and returns −1.
3. **An empty dispenser clicks (1001) and makes the game event `block_activate`; an empty
   dropper clicks only.** DispenserBlock.dispenseFrom 59–89 vs DropperBlock.dispenseFrom
   59–65.
4. **The sound and the smoke come after the item's behaviour, from the behaviour.**
   `DefaultDispenseItemBehavior.dispense` (11–30) is `execute`, then `playSound` (level
   event 1000, data 0, at the dispenser) and `playAnimation` (level event 2000 with
   `facing.get3DDataValue()`). An `OptionalDispenseItemBehavior` plays 1000 on success and
   1001 on failure; a projectile 1002 or its own override. **Several behaviours fall back
   to a whole inner `dispense`, and so play the sound and smoke twice**: a bucket that
   cannot be emptied, a boat or cart with nowhere to go, a potion other than water
   (4.5, 4.7, 4.8, 4.14). The client hears both; the port sends both.
5. **26.3 registers 82 items by name and finds the rest by component** (4.4). An item
   with `minecraft:equippable` (all armour, elytra, the shield, heads and skulls,
   saddles, carpets, harnesses, horse and wolf and nautilus armour) goes on the first
   `LivingEntity` in the block in front that can wear it, **players and armour stands
   included**, else is dropped. A creative player standing in front of a dispenser of
   armour gets it put on (4.9).
6. **Fire charges are projectiles in 26.3** (`registerProjectileBehavior(FIRE_CHARGE)`,
   DispenseItemBehavior.txt 33–34): a dispenser does not set fire with them; it shoots a
   small fireball (N9). Flint and steel lights fire, candles, campfires and TNT (4.11).
7. **The sulfur cube is a stable 26.3 mob** (`net.minecraft.world.entity.monster.cubemob.
   SulfurCube`; its archetypes are in `data/minecraft/sulfur_cube_archetype/`, the biome
   `sulfur_caves` in the main data, no experiment). Dispensers feed it blocks of
   `#sulfur_cube_swallowable` (TNT among them: the `explosive` archetype, fuse 120, power
   3), prime it with flint and steel, and fill or empty `sulfur_cube_bucket`. All of that
   is N7's (the mob); N6 needs only that a dispenser with no cube in front behaves as
   here (4.4 rule 2, 4.10, 4.11).
8. **A dispensed TNT is primed only if the game rule `tnt_explodes` is on**; off, it stays
   in the dispenser with the failure click (`$6`, lines 9–30). The rule is new; its
   default is in `GameRules`, not read (**Inferred** true). The PrimedTnt is made at the
   front block's `(x + 0.5, y, z + 0.5)` with `owner = null`, one level draw for its
   direction (4.10). Flint and steel primes a TNT *block* in front by `TntBlock.prime`
   and removes it (4.11).
9. **A dropper puts one item into the container in front, silently; if that fails it
   keeps the item** (DropperBlock.txt 28–121). Only when there is no container in front
   does it drop the item (1000 and 2000 then). A container entity in the box of the front
   block (a chest or hopper cart, a chest boat) counts, chosen by `nextInt(count)` of the
   level's random (HopperBlockEntity.txt 939–982). A dropper never uses the item's own
   behaviour: TNT, buckets, boats from a dropper are dropped as items.
10. **Inserting resets a hopper's cooldown as a hopper's own push does**
    (`tryMoveInItem`, 699–793: an empty hopper that is not on a custom cooldown gets
    cooldown 8, or 7 if it has already ticked this game tick). Section 5's rule, met here.
11. **A dispensed item entity draws four times from the level and its own six from its
    own random** (4.3); the shapes of motion are exact (`triangle` around `step · s` with
    deviation `0.0172275 · 6`), a vertical dispenser's item rises by `triangle(0.2, …)`
    like a horizontal one's.
12. **What the creative player meets that is N6's**: items dropped, armour put on players
    and armour stands, TNT primed, fire and candles lit by flint and steel, water and lava
    placed and picked up by buckets (and powder snow), boats and minecarts placed, armour
    stands placed, shulker boxes placed, carved pumpkins and wither skulls placed (the golem
    and wither checks are N7's), honeycomb waxing copper, glass bottles filled from water
    (no block change), water bottles making mud, glowstone charging a respawn anchor,
    shears cutting leashes (N5's leashes) and harvesting beehives. Waiting for later steps:
    every projectile (N9), spawn eggs (N7), bone meal (its growth, not read: 4.12), the
    brush on armadillos and shears on sheep and the like (N7), chests on llamas and donkeys
    (N7), golems and withers (N7/N10), sulfur cubes (N7).

### 4.1 Block, state and block entity

- `DispenserBlock extends BaseEntityBlock` with `FACING` (`DirectionalBlock.FACING`, six
  directions; default north) and `TRIGGERED` (default false) (static 405–421, constructor
  39–60, `createBlockStateDefinition` 388–404). `TRIGGER_DURATION = 4`.
- Placing: `getStateForPlacement` (284–295) sets `FACING` to the opposite of
  `getNearestLookingDirection()`, `TRIGGERED` false (Clustine's placement rule
  `AwayFromLook` already does this, `placement/rules.rs` 114). `rotate`/`mirror` turn
  `FACING`.
- Clicking: `useWithoutItem` (61–90): on the server, if the block entity is a
  `DispenserBlockEntity`, `player.openMenu(blockEntity)` and award `INSPECT_DROPPER` or
  `INSPECT_DISPENSER`; `SUCCESS` either way. The menu is `DispenserMenu`
  (`createMenu`, DispenserBlockEntity.txt 188–197): `MenuType.GENERIC_3x3`, nine
  container slots (3 × 3 at 62 + 18 · column, 17 + 18 · row), then the player's 27 + 9
  (`addStandardInventorySlots(inventory, 8, 84)`); its quick move and the click handling
  are section 3's. The title is `container.dispenser` / `container.dropper` unless named.
- Breaking: `affectNeighborsAfterRemoval` (296–303) is
  `Containers.updateNeighboursAfterDestroy` (the comparator's update); the contents are
  spilled by the container block entity's removal side effects (section 3). The block's
  loot table (`blocks/dispenser`, `blocks/dropper`) drops the block with its custom name
  and `survives_explosion` (section 2).
- Comparator: `hasAnalogOutputSignal` true; `getAnalogOutputSignal` (354–361) is
  `AbstractContainerMenu.getRedstoneSignalFromBlockEntity(be)` (section 3: the fullness
  of the nine slots).
- `DispenserBlockEntity extends RandomizableContainerBlockEntity`
  (DispenserBlockEntity.txt):
  `CONTAINER_SIZE = 9`, `items` a `NonNullList` of 9 (9–21). `loadAdditional` (139–158):
  the base's (name, lock), the items cleared, then `LootTable`/`LootTableSeed` if present
  (`tryLoadLootTable`), else `ContainerHelper.loadAllItems` (`Items`, each with `Slot`).
  `saveAdditional` (160–173) mirrors it. The dropper's entity (`DropperBlockEntity`) is the
  same with type `dropper` and name `container.dropper`.
- **`insertItem(stack)`** (72–132): what a behaviour gives back that does not replace the
  slot (a filled bottle, a bucket of water from an empty one, ...). For each slot in
  order: if it is empty or the same item with the same components, move
  `min(stack.count, max − slot.count)` (with `max = getMaxStackSize(stack)`,
  **Inferred** `min(container max 99, stack max)`, `Container.getMaxStackSize` not
  read): into an empty slot by `setItem(i, stack.split(k))`, into a matching one by
  shrinking `stack` and growing the slot; stop once `stack` is empty. Returns what is
  left.
- **Loot tables**: a dispenser made by a structure (`chests/jungle_temple_dispenser`:
  1–2 rolls of 2–7 arrows) unpacks its table the first time a slot is chosen
  (`getRandomSlot` calls `unpackLootTable(null)` first) or the menu is opened. The unpack
  is in `RandomizableContainer`, not dumped (section 12).

### 4.2 The tick: `dispenseFrom(level, state, pos)` (DispenserBlock.txt 91–156)

```
be = level.getBlockEntity(pos, DISPENSER) or return (a warning)
source = BlockSource(level, pos, state, be)      // state: the tick's state, as passed
slot = be.getRandomSlot(level.getRandom())        // 4.0 item 2: unpack, then one nextInt
per non-empty slot
if slot < 0:
    levelEvent(1001, pos, 0)                      // SOUND_DISPENSER_FAIL
    gameEvent(BLOCK_ACTIVATE, pos, Context.of(be.getBlockState()))
    return
stack = be.getItem(slot)                           // the slot's own stack, not a copy
behaviour = getDispenseMethod(level, stack)
if behaviour != NOOP:
    be.setItem(slot, behaviour.dispense(source, stack))
```

`getDispenseMethod` (157–179): an item not enabled by the level's feature flags →
`DEFAULT_BEHAVIOR`; else `DISPENSER_REGISTRY.get(item)` (an `IdentityHashMap`) if present;
else `getDefaultDispenseMethod` (180–206): `minecraft:equippable` →
`EquipmentDispenseItemBehavior.INSTANCE`; else `#minecraft:sulfur_cube_swallowable` →
`SulfurCubeBlockDispenseItemBehavior.INSTANCE`; else a `SpawnEggItem` with
`minecraft:entity_data` → `SpawnEggItemBehavior.INSTANCE`; else `DEFAULT_BEHAVIOR`. `NOOP`
(the identity lambda, DispenseItemBehavior.txt 329–341) is registered for nothing by
`bootStrap`; nothing reaches it today (**Inferred**: no other caller of `registerBehavior`
in the jar; not searched).

The behaviour mutates `stack` (the slot's object) and returns what goes back into the slot:
the same object shrunk, or a remainder (a bucket, a bottle) where the stack was the last
one. `setItem` marks the entity changed (the comparator's update, section 3).

`getDispensePosition(source)` (304–348) is `center + 0.7 · step(facing)`
(`Vec3.atCenterOf(pos)` plus `0.7 * stepX` and so on, in `double`).

### 4.3 The default behaviour: drop one (DefaultDispenseItemBehavior.txt)

`execute(source, stack)` (32–55): `dir = state.FACING`; `p = getDispensePosition(source)`;
`one = stack.split(1)`; `spawnItem(level, one, 6, dir, p)`; return `stack`.

`spawnItem(level, stack, accuracy, dir, p)` (57–135), exactly:

```
x = p.x; y = p.y; z = p.z
y -= (dir.axis == Y) ? 0.125 : 0.15625
item = new ItemEntity(level, x, y, z, stack)         // its own random: uuid (2
nextLong), bob, yaw, motion (n4 8.4)
r = level.getRandom()
s = r.nextDouble() * 0.1 + 0.2                        // level draw 1
item.setDeltaMovement(
    r.triangle(dir.stepX * s, 0.0172275 * accuracy),  // level draws 2-3 (Inferred two nextDouble)
    r.triangle(0.2,           0.0172275 * accuracy),  // level draws 4-5
    r.triangle(dir.stepZ * s, 0.0172275 * accuracy))  // level draws 6-7
level.addFreshEntity(item)
```

The constructor's motion is replaced; its draws are made all the same (Clustine's
`spawn_item_as` with a `motion` does this, `region/drops.rs` 136–175). No
`setDefaultPickUpDelay`: the item keeps the constructor's delay (0, as
`Containers.dropItemStack`'s, `drops.rs`). `0.0172275 * 6` is a `double` product computed
at run time (`ldc2_w 0.0172275`, `i2d`, `dmul`). `triangle(a, b)` is **Inferred**
`a + b · (nextDouble() − nextDouble())` (RandomSource not read, as in N5). Then
`dispense` plays 1000 and 2000 (4.0 item 4).

`consumeWithRemainder(source, stack, remainder)` (173–188): `stack.shrink(1)`; if now
empty return `remainder` (it takes the slot); else `addToInventoryOrDispense(remainder)`
(190–220): `be.insertItem(remainder)`; whatever does not fit is dropped by `spawnItem(…,
6, facing, getDispensePosition)` **with its own 1000 and 2000** (so a full dispenser that
fills a bottle plays the sound twice); return `stack`.

### 4.4 What `bootStrap` registers (DispenseItemBehavior.txt 9–321)

In order (the map is by identity; order matters only in that a later entry would
replace an earlier):

| Items | Behaviour | Section |
|---|---|---|
| arrow, tipped_arrow, spectral_arrow, egg, blue_egg, brown_egg, snowball, experience_bottle, splash_potion, lingering_potion, firework_rocket, fire_charge, wind_charge | `ProjectileDispenseBehavior(item)` | 4.17 (N9) |
| armor_stand | `$1` | 4.6 |
| chest | `$2` (chest on a tamed chested horse) | 4.16 (N7) |
| the 10 boats (oak, spruce, birch, jungle, dark_oak, acacia, cherry, mangrove, **poplar**, pale_oak), bamboo_raft, the 10 chest boats and bamboo_chest_raft | `BoatDispenseItemBehavior(type)` | 4.7 |
| lava_bucket, water_bucket, powder_snow_bucket, salmon_bucket, cod_bucket, pufferfish_bucket, tropical_fish_bucket, axolotl_bucket, sulfur_cube_bucket, tadpole_bucket | `$3` (one shared instance) | 4.5 |
| bucket | `$4` | 4.5 |
| flint_and_steel | `FlintAndSteelDispenseItemBehavior` | 4.11 |
| bone_meal | `$5` | 4.12 |
| tnt (`Blocks.TNT` as an item) | `$6` | 4.10 |
| wither_skeleton_skull | `$7` | 4.13 |
| carved_pumpkin (`Blocks.CARVED_PUMPKIN`) | `$8` | 4.13 |
| shulker_box and the 16 dyed (`Items.DYED_SHULKER_BOX.forEach`) | `ShulkerBoxDispenseBehavior` (one instance) | 4.15 |
| glass_bottle | `$9` | 4.14 |
| glowstone | `$10` | 4.14 |
| shears | `ShearsDispenseItemBehavior` | 4.16 |
| brush | `$11` (armadillo scutes) | 4.16 (N7) |
| honeycomb | `$12` | 4.14 |
| potion | `$13` | 4.14 |
| minecart, chest_minecart, furnace_minecart, tnt_minecart, hopper_minecart, command_block_minecart | `MinecartDispenseItemBehavior(type)` | 4.8 |

Then by component (4.2): equippable → 4.9; sulfur-cube-swallowable → 4.10 rule; spawn eggs
with entity data → 4.16; anything else → 4.3. The behaviour objects are singletons with
state: `OptionalDispenseItemBehavior.success` is a field of the shared instance, set by
each `execute` before `playSound` reads it (`$2` never sets it, so it stays at its initial
`true`, OptionalDispenseItemBehavior.txt 5–12).

### 4.5 Buckets (`$3`, `$4`)

**Full buckets** (`$3`, DispenseItemBehavior$3.txt 16–61): `target = pos + facing`;
`((DispensibleContainerItem) item).emptyContents(null, level, target, null)`; if true:
`checkExtraContent(null, level, stack, target)` (the fish or axolotl or tadpole or sulfur
cube released, N7) and `consumeWithRemainder(source, stack, new ItemStack(BUCKET))`
(the empty bucket takes the slot if it was the last; otherwise goes into the dispenser
or is dropped). If false: the inner default's whole `dispense(source, stack)` (drop one,
1000 + 2000), then the outer `dispense` plays 1000 + 2000 again. `emptyContents` is
`BucketItem`'s (not read: placing the fluid as a player's bucket does, waterlogging,
evaporation in an ultrawarm dimension, the empty sound, `FLUID_PLACE`); Clustine's
`placement/bucket.rs` has the player's version to share.

**The empty bucket** (`$4`, 9–73): `target = pos + facing`; `b =
level.getBlockState(target)`;
if its block is a `BucketPickup`: `filled = pickupBlock(null, level, target, b)`; empty
result → the default `execute` (drop the bucket, one item); else
`gameEvent(null, FLUID_PICKUP, target)` and `consumeWithRemainder(source, stack, new
ItemStack(filled.getItem()))`. Not a `BucketPickup` → the default `execute` (drops the
bucket). `pickupBlock` (liquid blocks' source check, waterlogged blocks drained, powder
snow) is not read (section 12); the sound of filling is the block's
`getPickupSound`, played by `BucketItem` for a player but **not here** (**Inferred** from
this code: no `playSound` in `$4`). Only one 1000 + 2000 here (`$4` calls `super.execute`,
not `dispense`).

### 4.6 Armour stand (`$1`, DispenseItemBehavior$1.txt 9–56)

`target = pos + facing`; `processor = appendDefaultStackConfig(stand → stand.setYRot(
facing.toYRot()), level, stack, null)`; `ARMOR_STAND.spawn(level, processor, target,
DISPENSER, false, false)`; if non-null, `stack.shrink(1)`. `EntityType.create` with the
processor (EntityType.txt 221–316): `create(level, reason)`; no Y offset (both flags
false); `snapTo(target.x + 0.5, target.y, target.z + 0.5, wrapDegrees(level.random.
nextFloat() * 360f), 0)` (**one level draw**, discarded by the processor's yaw); not a
`Mob`; then the processor: the stack's default config (custom name, entity data:
N5's 5.1) and `setYRot(facing.toYRot())`. `spawn` (194–220) adds it with its passengers.
**No collision or space check**: the stand is placed into whatever is in front, solid
blocks included. `toYRot` of `up`/`down` is **Inferred** `(get2DDataValue() & 3) * 90`
(`Direction` not read). Sound and smoke: the default's 1000 + 2000.

### 4.7 Boats (`BoatDispenseItemBehavior`, 153 lines)

`execute` (21–141), with `c = Vec3.atCenterOf(pos)` and `w = type.getWidth()` (a float,
1.375 for boats and rafts, N5 4.8):

```
k  = 0.5625 + (double) w / 2.0                         // f2d, then ddiv
x  = c.x + stepX * k
y  = c.y + (double)((float) stepY * 1.125f)            // float product, widened
z  = c.z + stepZ * k
front = pos + facing
if level.getFluidState(front).is(#water):          lift = 1.0
elif level.getBlockState(front).isAir() and
level.getFluidState(front.below()).is(#water): lift = 0.0
else: return inner.dispense(source, stack)              // drop one: 1000 + 2000 twice in
all
boat = type.create(level, DISPENSER)
if boat != null:
    boat.setInitialPos(x, y + lift, z)
    createDefaultStackConfig(level, stack, null).apply(boat)
    boat.setYRot(facing.toYRot())
    level.addFreshEntity(boat); stack.shrink(1)
return stack
```

`playSound` (143–152) is always 1000. **No collision check**: unlike `BoatItem.use` (N5
4.1), the boat is made even inside blocks or other entities. A horizontal dispenser above
water puts the boat 1.25 blocks out at its own centre height plus one (`c.y + 1`) when the
front block holds water, or at `c.y` over air with water below; then it falls or floats
(N5 4.2). Clustine's `place_boat` (region/boats.rs 333) is the player's; the dispenser's
needs its own path with the same entity data.

### 4.8 Minecarts (`MinecartDispenseItemBehavior`, 187 lines)

`execute` (21–155), `c = Vec3.atCenterOf(pos)`:

```
x = c.x + stepX * 1.125
y = Math.floor(c.y) + stepY                            // the dispenser's own y plus the
step
z = c.z + stepZ * 1.125
front = pos + facing; b = getBlockState(front)
if b.is(#rails):            off = railShape(b).isSlope() ? 0.6 : 0.1
elif b.isAir():
    below = getBlockState(front.below())
    if below.is(#rails):    off = (facing != DOWN and railShape(below).isSlope()) ? -0.4 : -0.9
    else: return inner.dispense(source, stack)
else: return inner.dispense(source, stack)
cart = AbstractMinecart.createMinecart(level, x, y + off, z, type, DISPENSER, stack, null)
if cart != null: level.addFreshEntity(cart); stack.shrink(1)
```

`getRailShape` (157–175): a `BaseRailBlock`'s shape property, else `NORTH_SOUTH`.
`playSound` (177–186) is always 1000. `createMinecart` is N5's 3.2 (Clustine's
`place_minecart`, region/minecarts.rs 274, computes the player's position; the
dispenser's differs: 1.125 out, and these offsets). Under the old behaviour the cart then
snaps to the rail in its first tick (N5 3.4). The `command_block_minecart` is placed too
(its behaviour is N5's "kinds left", 3.19).

### 4.9 Equipment (`EquipmentDispenseItemBehavior`, 96 lines)

`execute` (11–23): `dispenseEquipment(source, stack)` ? `stack` : the default `execute`
(drop one). `dispenseEquipment` (25–80):

```
front = pos + facing
list = level.getEntitiesOfClass(LivingEntity, new AABB(front), e ->
e.canEquipWithDispenser(stack))
if list.isEmpty(): return false
e = list.getFirst()
slot = e.getEquipmentSlotForItem(stack)               // the equippable's slot if
e.canUseSlot(it), else MAINHAND
e.setItemSlot(slot, stack.split(1))
if e is Mob: setGuaranteedDrop(slot); setPersistenceRequired()
return true
```

`canEquipWithDispenser` (LivingEntity.txt 10680–10729): alive and not a spectator; the
stack has `equippable` with `dispensable` (every item in 26.3's data has it true); the
entity `canUseSlot(slot)` (an armour stand refuses `body`, `saddle` and slots it has
disabled, ArmorStand.txt 201–227; a player and other living entities accept all); the
equippable's `allowed_entities`, if any, holds the entity's type; the slot is **empty**;
`canDispenserEquipIntoSlot(slot)`: true for players and armour stands (not overridden),
`canPickUpLoot()` for mobs. So a creative player in front of a dispenser of armour has
it put on (when the slot is empty), and so does an armour stand. `new AABB(front)` is the
unit box of the front block; `getEntitiesOfClass` returns entities whose box meets it
(order: the entity sections', N5's open question). Sound: 1000 + 2000 either way (the
default `dispense`). The player's change of armour goes to the player's inventory (their
armour slots, `clustine_world::ARMOUR_START`) and is shown to others as equipment; the
stand's as N5's `SetEquipment`.

### 4.10 TNT (`$6`, 91 lines) and swallowable blocks

```
if !level.gameRules.get(TNT_EXPLODES): setSuccess(false); return stack     // 1001, item
kept
front = pos + facing
if SulfurCubeBlockDispenseItemBehavior.dispenseBlock(level, front, stack): return stack
// a cube took it (success stays as last set!)
t = new PrimedTnt(level, front.x + 0.5, front.y, front.z + 0.5, null)     //
PrimedTnt.txt 43-90: one level nextDouble
level.addFreshEntity(t)
level.playSound(null, t.x, t.y, t.z, TNT_PRIMED, BLOCKS, 1f, 1f)
level.gameEvent(null, ENTITY_PLACE, front)
stack.shrink(1); setSuccess(true)
```

(lines 9–90). The PrimedTnt constructor: `setPos`; `a = level.random.nextDouble() *
6.2831854820251465`; motion `(-Math.sin(a) * 0.02, 0.20000000298023224, -Math.cos(a) *
0.02)` (`java.lang.Math.sin`/`cos`, not the game's table: ADR-0028 ports them with
`StrictMath`); fuse 80;
`xo/yo/zo` set; owner `null` (section 1). The dispenser's TNT does **not** go through
`TntBlock.prime` (no `PRIME_FUSE` game event, no adventure check): it makes
`ENTITY_PLACE` at the front block. The TNT is placed into whatever is in front (no space
check). Where a sulfur cube took the block, `success` is not set by this call
(it keeps the instance's last value: **a quirk**, the click is 1000 or 1001 by what the
previous TNT dispensed from any dispenser did; with no cubes in N6 it is moot).

`SulfurCubeBlockDispenseItemBehavior` (72 lines): for a swallowable block item not
otherwise registered: `dispenseBlock(level, front, stack)` (34–63: the cubes whose box
meets `AABB(front)`, the first whose `equipItem(stack)` is true takes one) or the default
drop. With no cubes it is the default drop.

### 4.11 Flint and steel (`FlintAndSteelDispenseItemBehavior`, 139 lines)

`execute` (9–109), `setSuccess(true)` first; `front = pos + facing`; `b = state at front`:

1. `tryIgniteExplosiveEntities(level, front)` (111–134): the first `SulfurCube` in
   `AABB(front)` (filtered by a lambda, not shown) gets `primeTime(false)` → done
   (N7; none in N6).
2. else if `BaseFireBlock.canBePlacedAt(level, front, facing)`:
   `setBlockAndUpdate(front, BaseFireBlock.getState(level, front))` (fire or soul fire)
   and `gameEvent(null, BLOCK_PLACE, front)`. Fire's own `onPlace` (its scheduled tick,
   a nether portal) follows as for any fire (Clustine's `random_ticks/fire.rs`).
3. else if `CampfireBlock.canLight(b) || CandleBlock.canLight(b) ||
   CandleCakeBlock.canLight(b)`: `setBlockAndUpdate(front, b.setValue(LIT, true))` and
   `gameEvent(null, BLOCK_CHANGE, front)`.
4. else if `b.getBlock() instanceof TntBlock`: `TntBlock.prime(level, front, null,
   stack)` (TntBlock.txt 140–221: false when `tnt_explodes` is off; else a PrimedTnt at
   `(x + 0.5, y, z + 0.5)`, one level draw, `TNT_PRIMED` at the entity, `PRIME_FUSE` game
   event at the block) and, if true, `removeBlock(front, false)`; if false
   `setSuccess(false)`.
5. else nothing (`success` stays true: **the click is 1000 though nothing happened**).

Then if `success`: `stack.hurtAndBreak(1, level, null, onBreak)` (one point of
durability; breaks at the end, `ItemStack.hurtAndBreak` not read: **Inferred** no draw
without Unbreaking). Sound by success (1000/1001); smoke 2000. The flint and steel stays
in the slot until broken. `canBePlacedAt`, `getState` and the `canLight`s are not read
(section 12); Clustine's fire code knows the fire's state.

### 4.12 Bone meal (`$5`, 51 lines)

`setSuccess(true)`; `front = pos + facing`; if `BoneMealItem.growCrop(stack, level,
front)` or `BoneMealItem.growWaterPlant(stack, level, front, null)`: `levelEvent(1505,
front, 15)` (`PARTICLES_AND_SOUND_PLANT_GROWTH`, data 15); else `setSuccess(false)`.
`BoneMealItem` is not dumped: what grows, how (crops, saplings by their tree features,
grass, flowers, coral, sea grass: the `BonemealableBlock`s) and its draws are unknown here.
Recommendation: leave to the step that brings bone meal to the player (a creative player
uses it by hand far more than by dispenser); until then a dispenser with bone meal fails
(1001, item kept), which is `$5`'s behaviour where nothing can grow (**a named
difference**).

### 4.13 Wither skeleton skull (`$7`) and carved pumpkin (`$8`)

`$7` (79 lines): `front = pos + facing`; if `isEmptyBlock(front)` and
`WitherSkullBlock.canSpawnMob(level, front, stack)` (the soul sand T is present): set a
`wither_skeleton_skull` with `ROTATION = RotationSegment.convertToSegment(facing)`,
`BLOCK_PLACE`, and if its block entity is a skull `WitherSkullBlock.checkSpawn` (the
wither: N9/N10); shrink, success. Otherwise `setSuccess(dispenseEquipment(source,
stack))` (4.9: onto a head). `$8` (63 lines): the same with `CarvedPumpkinBlock.
canSpawnGolem(level, front)` (a snow or iron golem's body below) and the pumpkin's
default state; the golem itself is made by the pumpkin's `onPlace` (N7). Otherwise
equipment. In N6 without golems or withers both are "put on a head or fail" (1001, item
kept): `canSpawnMob`/`canSpawnGolem` need the patterns (not read); a port can answer
false until N7, a **named difference** only when a player builds a golem in front of a
dispenser.

### 4.14 Bottles, glowstone, honeycomb, potions

- **Glass bottle** (`$9`, 116 lines): `setSuccess(false)`; `front`; if the block is in
  `#beehives` with `honey_level` and is a `BeehiveBlock` with level ≥ 5:
  `releaseBeesAndResetHoneyLevel(level, b, front, null, BEE_RELEASED)` (bees: N7),
  success, `takeLiquid(source, stack, honey_bottle)`; else if `getFluidState(front).
  is(#water)` (source or flowing): success, `takeLiquid(source, stack, potion of water)`
  (the water is **not** removed); else the default `execute` with success false (**the
  bottle is dropped and the click is the failure's 1001**). `takeLiquid` (9–23):
  `gameEvent(null, FLUID_PICKUP, source.pos)` (**at the dispenser**, not the front) and
  `consumeWithRemainder`.
- **Glowstone** (`$10`, 62 lines): `setSuccess(true)`; at a `respawn_anchor` in front:
  charge below 4 → `RespawnAnchorBlock.charge(null, level, front, b)` and shrink; at 4 →
  failure. Not an anchor → the default `execute` (drop, success true: 1000).
  `RespawnAnchorBlock.charge` (not read; Clustine has the anchor's comparator output) sets
  `charge + 1`, plays its sound and makes a game event (**Inferred**).
- **Honeycomb** (`$12`, 66 lines): `HoneycombItem.getWaxed(b)` present →
  `setBlockAndUpdate(front, waxed)`, `levelEvent(3003, front, 0)` (`PARTICLES_WAX_ON`),
  `playSound(null, front, HONEYCOMB_WAX_ON, BLOCKS, 1, 1)`, shrink, success; else the
  default `execute` (drop, 1000). `getWaxed` is the copper blocks' waxing map (not read;
  section 12).
- **Potion** (`$13`, 128 lines): not a water potion → the inner default's whole
  `dispense` (drop: 1000 + 2000, then again). A water potion: if the front block is in
  `#convertible_to_mud`: five times `sendParticles(SPLASH, pos.x + nextDouble(),
  pos.y + 1, pos.z + nextDouble(), 1, 0, 0, 0, 1)` (**at the dispenser's own block's
  top, ten level draws**), `playSound(null, pos, BOTTLE_EMPTY, BLOCKS, 1, 1)`,
  `gameEvent(null, FLUID_PLACE, pos)` (both at the dispenser), `setBlockAndUpdate(front,
  mud)`, `consumeWithRemainder(source, stack, glass_bottle)`; else the inner default's
  `dispense` (drop, twice the sound).

### 4.15 Shulker boxes (`ShulkerBoxDispenseBehavior`, 78 lines)

`execute` (11–68): `setSuccess(false)`; for a `BlockItem`: `front = pos + facing`;
`dir = isEmptyBlock(front.below()) ? facing : UP`; `setSuccess(blockItem.place(new
DirectionalPlaceContext(level, front, facing, stack, dir)).consumesAction())` (an
exception is logged and counts as failure). **A box that cannot be placed stays in the
dispenser** (1001; not dropped). `BlockItem.place` runs the ordinary placement (its
`canPlace`, the block's `getStateForPlacement` with the context's face `dir` →
`ShulkerBoxBlock`'s facing, the block entity's data from the stack, the place sound, the
item shrunk). `DirectionalPlaceContext` is not read (**Inferred**: replaces only
replaceable blocks, `getNearestLookingDirection` = `facing`, clicked face = `dir`).
Clustine's block placing (`placement`) is the player's; the dispenser's needs a context
without a player.

### 4.16 Shears, brush, chest on a horse, spawn eggs (mostly later steps)

- **Shears** (`ShearsDispenseItemBehavior`, 182 lines): `front`; `tryShearBeehive`
  (53–107: a beehive with honey 5: `BEEHIVE_SHEAR` sound at `front`,
  `BeehiveBlock.dropHoneycomb`, `releaseBeesAndResetHoneyLevel`, `SHEAR` game event)
  or `tryShearEntity` (109–162): for each entity (any class, `NO_SPECTATORS`) in
  `AABB(front)`: **`shearOffAllLeashConnections(null)` true → done** (N5's leashes:
  a leashed boat in front is let go, the lead dropped); else a live `Shearable` ready
  for shearing → `shear(level, BLOCKS, stack)` and `SHEAR` (N7). Success → one point of
  durability. **The leash part is N6's** (N5's `Leashable` is built).
  `Entity.shearOffAllLeashConnections` is in Entity.txt (not read for this section:
  section 12).
- **Brush** (`$11`): the first armadillo in `AABB(front)` whose `brushOffScute(null,
  stack)` is true; 16 durability. N7. Without one: failure (1001, item kept).
- **Chest** (`$2`): a tamed `AbstractChestedHorse` without a chest in `AABB(front)` gets
  it in slot 499 (`getSlot(499).set(stack)`); else dropped (success stays true). N7.
- **Spawn eggs** (`SpawnEggItemBehavior`, 82 lines): `type = SpawnEggItem.getType(stack)`
  (null → return stack); `type.spawn(level, stack, null, front, DISPENSER, facing != UP,
  false)`; null → return stack unchanged; on success shrink and `gameEvent(null,
  ENTITY_PLACE, pos)` (at the dispenser). N7.

### 4.17 Projectiles (N9: names only)

`ProjectileDispenseBehavior(item)` (103 lines): the item must be a `ProjectileItem`; its
`createDispenseConfig()` gives a position function, `power`, `uncertainty` and an optional
override of the launch level event. `execute` (36–84): `p =
config.positionFunction().getDispensePosition(source, facing)`;
`Projectile.spawnProjectileUsingShoot(item.asProjectile(level, p, stack, facing), level,
stack, stepX, stepY, stepZ, power, uncertainty)`; shrink. `playSound` (86–102):
`levelEvent(override or 1002, pos, 0)`. The items: arrow, tipped_arrow, spectral_arrow,
egg, blue_egg, brown_egg, snowball, experience_bottle, splash_potion, lingering_potion,
firework_rocket, fire_charge, wind_charge. Until N9 a dispenser with any of these could
drop it as an item (**a named difference**: a creative player notices at once, since
arrows and snowballs fly from a dispenser) or refuse (1001, item kept). Recommendation:
**drop them** (the default behaviour), named in the roadmap, since the item at least
leaves; or bring the projectiles' flight forward for the dispenser alone (their entities
without hitting, which is most of N9).

### 4.18 The dropper (DropperBlock.txt, DropperBlockEntity.txt)

`DropperBlock extends DispenserBlock`: its state, placing, power rule and tick are the
dispenser's; `getDispenseMethod` (14–17) is always its own `DefaultDispenseItemBehavior`;
`newBlockEntity` a `DropperBlockEntity`. `dispenseFrom(level, state, pos)` (28–121):

```
be = level.getBlockEntity(pos, DROPPER) or return (a warning)
source = BlockSource(level, pos, state, be)
slot = be.getRandomSlot(level.getRandom())                    // as the dispenser's
if slot < 0: levelEvent(1001, pos, 0); return                  // no game event
stack = be.getItem(slot)
if stack.isEmpty(): return                                     // cannot happen
facing = level.getBlockState(pos).FACING                        // read again from the level
into = HopperBlockEntity.getContainerAt(level, pos + facing)
if into == null:
    rest = DISPENSE_BEHAVIOUR.dispense(source, stack)            // drop one: 4.3, 1000 + 2000
else:
    left = HopperBlockEntity.addItem(be, into, stack.copyWithCount(1), facing.opposite)
    rest = left.isEmpty() ? stack.copy() shrunk by 1 : stack.copy()
be.setItem(slot, rest)
```

`getContainerAt(level, p)` (HopperBlockEntity.txt 845–869, 870–887): first the block's
container (`getBlockContainer`, 888–938): a `WorldlyContainerHolder` block's
(the composter), else a block entity that is a `Container` (a chest's through
`ChestBlock.getContainer(…, true)`: the double chest as one container); if none, the
container entities (`EntitySelector.CONTAINER_ENTITY_SELECTOR`: chest and hopper carts,
chest boats and rafts, **Inferred** alive) whose box meets the box of half-size 0.5
around `p`'s centre (939–982); several → `list.get(level.random.nextInt(size))` (**a
level draw**). `addItem(from, to, stack, face)` (588–645): a `WorldlyContainer` tries the
slots `getSlotsForFace(face)` in order, others all slots in order, until the stack is
empty, each by `tryMoveInItem` (699–793): `canPlaceItem` (and `canPlaceItemThroughFace`)
of the slot; into an empty slot whole, else merged up to the stack's maximum if
`canMergeItems` (same item, same components, count below maximum); a hopper that was
empty and is not on its custom cooldown gets cooldown 8 (or 7 when it has ticked this
game tick: `tickedGameTime` compared); `setChanged` on the receiver. So a dropper facing a
hopper refills it and restarts its cooldown; facing a full container it **keeps the item
and makes no sound**. Section 5 has the hopper's side; section 3 the containers'
`canPlaceItem` (a shulker box refuses shulker boxes, ...).

### 4.19 What exists in Clustine to reuse, and what is missing

- **Power and the tick**: `region/redstone.rs` has `redstone_neighbour_changed` (43) with
  `has_neighbour_signal` (217) and the block ticks (`redstone_tick`, 373;
  `region/ticks.rs` `schedule_tick`, 184): the dispenser, dropper and crafter are a new
  case there (rising edge, `triggered`, delay 4), with flags 2 as the existing blocks
  use. The quasi-connected read of `pos.above()` is the piston's (`region/pistons.rs`).
- **Typed block entities**: `region/block_entities.rs` has `Typed` (36) for the bell,
  comparator and moving piston. Dispensers, droppers and crafters need a typed container
  (nine `ItemStack`s, the name, `LootTable`/`LootTableSeed`; the crafter's extra state,
  7.3), read from and written to the chunk's block-entity data in the game's NBT
  (`Items` with `Slot`), as section 3's containers need.
- **Items**: `region/drops.rs` `spawn_item_as` (149) with an explicit motion and a delay
  of 0 is `spawnItem` once the four level draws are drawn from the block's stream (n4
  8.5's proposal); `region/items.rs` has the item's tick and pick-up.
- **Boats and carts**: `region/boats.rs` `place_boat` (333) and `region/minecarts.rs`
  `place_minecart` (274) make the entities from a player's click; the dispenser's paths
  compute other positions and skip the boat's collision check, but share the entity's
  making (`kind_of_item`, `boat_of_item`, the entity data).
- **Buckets**: `placement/bucket.rs` has the player's bucket use (empty and filled); the
  dispenser's needs `emptyContents` and `pickupBlock` at a block without a ray.
- **Fire**: `region/random_ticks/fire.rs` has fire's ticks and `onPlace`'s parts; the
  dispenser needs `canBePlacedAt` and `getState`.
- **Armour stands** (`region/stands.rs`) and the player's inventory
  (`region/items.rs` `add_to_inventory`, `clustine_world::ARMOUR_START`) for equipment.
- **Level events**: `api.rs` has `RegionEvent::LevelEvent` (928); 1000, 1001, 1002, 2000,
  1505, 3003 are new numbers only.
- **Missing**: containers and menus (section 3: nothing opens a menu today),
  `HopperBlockEntity`'s container search and `addItem` (section 5 builds them), PrimedTnt
  (section 1), the game rule `tnt_explodes`.

## 5. Hoppers

Classes read: `HopperBlock` (HopperBlock.txt, 364 lines), `HopperBlockEntity`
(HopperBlockEntity.txt, 1150 lines), `Hopper` (Hopper.txt), `MinecartHopper`
(MinecartHopper.txt, 231 lines), `Container`, `BaseContainerBlockEntity`,
`RandomizableContainerBlockEntity`, `BlockEntity` (`setChanged`, `preRemoveSideEffects`),
`ShulkerBoxBlockEntity` and `CrafterBlockEntity` for their side rules, `EntitySelector`,
`Containers`, all in `n6-javap/`; `EntityGetter`, `AbstractMinecartContainer` in
`n5-javap/`. The hopper's menu (`HopperMenu`, five slots) is section 3's.

### 5.0 Findings of this section

1. **An item falling into a hopper is taken in the entity phase, and the hopper pushes
   then too.** `HopperBlock.entityInside` (an item entity's box walk meets the hopper's
   block) calls the same `tryMoveItems` as the ticker, with the supplier "take this one
   item entity": if the hopper is off cooldown and enabled it first **ejects** one item
   into its target (when not empty), then takes the entity, and goes on cooldown 8 if
   either moved. So a hopper can move items during the entities' tick, before its own
   ticker runs in the block entities' phase, and that ticker then finds it on cooldown
   (5.4, 5.8). Clustine's item walk (`physics/item.rs`, `ItemInside::met`) is pure today
   and reaches no block with a block entity: it must hand the hopper's position back to
   the region, as the cart's walk does (`CartInside`).
2. **The transfer cooldown is 8, handed on as 8 or 7.** Every hopper tick first lowers
   `cooldownTime` by one and stamps `tickedGameTime`; it moves only at `cooldownTime <=
   0`, and after any move sets 8. A hopper that pushes into an **entirely empty** hopper
   sets that hopper's cooldown to `8 - k`, where `k = 1` if the target has already ticked
   this game tick (`target.tickedGameTime >= source.tickedGameTime`), else 0, so a
   chain of hoppers moves an item one hopper every 8 ticks whatever their ticker order.
   A target on a cooldown above 8 (only from a saved `TransferCooldown`) is left alone.
   `tickedGameTime` is not saved: after a load it is 0 (5.3, 5.7).
3. **Entity containers are chosen by a draw.** Where no block container is at the target
   (or source) position, the hopper looks for entities that are `Container`s and alive
   (chest and hopper carts, chest boats and chest rafts) whose boxes meet the 1×1×1 box
   centred on that block's centre, and takes `list.get(level.random.nextInt(size))`:
   one draw of the level's random each time at least one is there, even when only one
   is, both for the eject target and the source above. This is the only draw of this
   section (5.6; section 10).
4. **Taking from an item entity is all or nothing for the return value.** `addItem(
   container, itemEntity)` puts a copy of the whole stack in, discards the entity when
   all of it went, and otherwise sets the entity's stack to what is left and returns
   **false**: the loop goes on to the next entity, and a hopper that took only part of
   every stack sets no cooldown and tries again next tick (5.5).
5. **Sucking stops at a block container even if nothing can be taken from it.** With a
   container above (a block's, or an entity's by the draw), the hopper tries only it,
   slot by slot, and never looks at item entities that tick. Without one, a full
   collision block above that is not in `#minecraft:does_not_block_hoppers` (only
   `#minecraft:beehives`) stops item suction; a hopper cart is never stopped (5.5).
6. **Comparator updates on attempts that fail.** Taking from a source is `removeItem(
   slot, 1)`, the add, and on failure the count restored and, when the stack had one
   item, `setItem` back. `BaseContainerBlockEntity.removeItem` and `setItem` (chest,
   barrel, dispenser, dropper, shulker box, crafter…) each call `setChanged`, which
   updates the comparators around (`updateNeighbourForOutputSignal`), so a chest above a
   hopper that cannot take its items tells its comparators every hopper tick per slot
   tried. The hopper's own `removeItem` and `setItem` are overridden **without**
   `setChanged`; the hopper calls it once per successful move. A minecart's
   `setChanged` does nothing (AbstractMinecartContainer.txt 72–74) (5.2, 5.4, 5.5).
7. **Pulling from a chest with a loot table generates its loot.** `RandomizableContainer
   BlockEntity.getItem`/`removeItem`/`setItem` call `unpackLootTable(null)` first, so
   a hopper under an unopened structure chest fills it from its loot table with no
   player (draws from `LootTableSeed`; section 3 has the unpacking).
8. **A hopper block entity sends nothing to clients.** `HopperBlockEntity` overrides
   neither `getUpdatePacket` nor `getUpdateTag`; its contents reach a client only
   through the menu (section 3). Its block state changes only for `ENABLED` and is sent
   as any block change.
9. **`ENABLED` is set with flag 2 only** (clients, no neighbour or shape updates) on
   every `neighborChanged` and on `onPlace` when the block was not already a hopper:
   `ENABLED = !hasNeighborSignal(pos)` (5.1).
10. **A hopper cart has no cooldown, never pushes, and takes once a tick.** It sucks after
    its move each tick (and, under the new behaviour only, after each step along the
    track), at most once per tick (`consumedItemThisFrame`), only while `enabled`, which
    an activator rail sets to `!powered` every tick the cart is on one. It is emptied
    only by a hopper below or beside it pulling or pushing through the entity search.
    Its `Enabled` is saved (default true) and not synched; Clustine's `EntityData::
    Minecart` has no field for it yet (5.9).
11. **Stack sizes**: a container's limit is `min(99, item's max stack size)`
    (`Container.getMaxStackSize`, Container.txt 17–29); `setItem` truncates a stack to
    it (`ItemStack.limitSize`), and the hopper's empty-slot path hands the whole
    incoming stack to `setItem`, so an over-size stack from an item entity loses its
    excess (only from commands). Clustine's `ItemStack` has an item and a count and no
    components: `isSameItemSameComponents` reduces to the item until components exist.

### 5.1 The block: `HopperBlock`

- **Properties**: `FACING` = `BlockStateProperties.FACING_HOPPER` (down, north, south,
  west, east: no up), `ENABLED` (static, 357–363). Default `FACING = DOWN, ENABLED = true`
  (constructor, 11–59).
- **Placing** (`getStateForPlacement`, 127–150): `dir = clickedFace.getOpposite()`;
  `FACING = dir.getAxis() == Y ? DOWN : dir`, `ENABLED = true` (so a click on a top or
  bottom face gives a hopper facing down; on a side, facing into that block).
- **Block entity**: `newBlockEntity` (152–159) makes a `HopperBlockEntity`. The ticker
  (`getTicker`, 161–172): on the server, `createTickerHelper(type, HOPPER,
  HopperBlockEntity::pushItemsTick)` (**Inferred** the method reference: the only static
  method of matching shape; no `javap -v` read).
- **`onPlace(state, level, pos, old, moved)`** (174–187): if `old` is the same block,
  nothing; else `checkPoweredState(level, pos, state)`.
- **`neighborChanged`** (214–221): `checkPoweredState(level, pos, state)`, whatever
  changed.
- **`checkPoweredState`** (223–251): `on = !level.hasNeighborSignal(pos)`; if `on !=
  ENABLED`: `level.setBlock(pos, state.setValue(ENABLED, on), 2)`. No sound, no event.
- **`useWithoutItem`** (189–212): on the server, if the block entity is a
  `HopperBlockEntity`, `player.openMenu(it)` and `awardStat(INSPECT_HOPPER)`; always
  `SUCCESS`. (Section 3.)
- **`affectNeighborsAfterRemoval`** (253–259): `Containers.updateNeighboursAfterDestroy(
  state, level, pos)` = `level.updateNeighbourForOutputSignal(pos, block)` only
  (Containers.txt 153–160). The contents drop earlier, by `BlockEntity.
  preRemoveSideEffects` (BlockEntity.txt 557–571): a `Container` block entity calls
  `Containers.dropContents(level, pos, container)` (N5 brief 3.18 has `dropContents`).
- **Comparator**: `hasAnalogOutputSignal` true; `getAnalogOutputSignal` (266–272) =
  `AbstractContainerMenu.getRedstoneSignalFromBlockEntity(level.getBlockEntity(pos))`
  (section 3; the repo has the formula as `physics::minecart::container_signal`).
- **`entityInside(state, level, pos, entity, applier, intersects)`** (317–335): if the
  block entity is a `HopperBlockEntity`, `HopperBlockEntity.entityInside(level, pos,
  state, entity, it)` (5.8). `HopperBlock` does not override
  `getEntityInsideCollisionShape`, so the walk calls it for every entity whose deflated
  box is in the hopper's block (N4 brief, `checkInsideBlocks`).
- **Shapes** (makeShapes 61–105, lambda 342–355; units of 1/16): `Block.column(w, y0,
  y1)` is a column `w` wide centred in the block from `y0` to `y1` (**Inferred** from
  its use; `Block` not read). Base = `column(16, 10, 16) ∪ column(8, 4, 10)`, less
  the bowl `column(12, 11, 16)` (`ONLY_FIRST`); the spout `boxZ(4, 4, 8, 0, 8)` (4 wide,
  y 4–8, z 0–8: **Inferred** argument order) rotated to `FACING` about (8, 6, 8)/16
  (`rotateAll`), cut to the block (`AND Shapes.block()`) and added. `getShape` ignores
  `ENABLED` (`getShapeForEachState(…, ENABLED)` lists it as ignored). Interaction shape
  (`getInteractionShape`, 116–125): the bowl `column(12, 11, 16)` plus, for a side
  facing, `boxZ(4, 8, 10, 0, 4)` rotated horizontally; for `DOWN` the bowl alone. The
  collision shape is the block shape (not overridden), so an item rests on the bowl's
  floor at y + 10/16 (a 0.25-high item box then reaches y + 0.875, above the suck box's
  floor at y + 11/16).
- `isPathfindable` false (337–340); `rotate`/`mirror` turn `FACING` (274–298).

### 5.2 The container: `HopperBlockEntity` state, save, slots

- Fields (1–19): `items` (5 stacks), `cooldownTime` (initially −1, `NO_COOLDOWN_TIME`),
  `tickedGameTime` (long, not saved), `facing` (cached from the state in the constructor
  (21–42) and in `setBlockState` (128–139), which the chunk calls when the state changes
  under a block entity it keeps).
- **Load** (44–69): items cleared to 5 empty; unless `tryLoadLootTable` (a `LootTable`
  key), `ContainerHelper.loadAllItems`; `cooldownTime = TransferCooldown` or −1.
- **Save** (71–89): unless `trySaveLootTable`, `ContainerHelper.saveAllItems`; always
  `TransferCooldown` (int). Name: `container.hopper` (static, 1141–1149), or a custom name
  (`BaseContainerBlockEntity`, section 3).
- **`removeItem(slot, n)`** (98–108): `unpackLootTable(null)`, then
  `ContainerHelper.removeItem(items, slot, n)` (a split of the stack in place: the stack
  in the slot keeps the same object with its count lowered). **No `setChanged`**.
- **`setItem(slot, stack)`** (110–126): `unpackLootTable(null)`, set, `stack.limitSize(
  getMaxStackSize(stack))`. **No `setChanged`**. (`BaseContainerBlockEntity`'s versions,
  BaseContainerBlockEntity.txt 166–206, call `setChanged` after a non-empty removal and
  after every set; `RandomizableContainerBlockEntity` (64–95) unpacks first and calls
  them.)
- **`setChanged()`** (BlockEntity.txt 484–511): `level.blockEntityChanged(pos)` (the
  chunk is to be saved) and, unless the cached state is air,
  `level.updateNeighbourForOutputSignal(pos, block)`. The hopper's static
  `setChanged(level, pos, state)` is the same with the given state.
- **Slots by face** (`getSlots(container, face)`, 319–357): a `WorldlyContainer` gives
  `getSlotsForFace(face)`; any other container `0 .. size-1` in order (cached for sizes
  below 54, `createFlatSlots` 359–377).
- **Worldly containers met**: `ShulkerBoxBlockEntity` (every face: all 27 slots;
  `canPlaceItemThroughFace` false for an item whose block is a `ShulkerBoxBlock`;
  `canTakeItemThroughFace` true; ShulkerBoxBlockEntity.txt 527–547), the composter
  (by `WorldlyContainerHolder`, not dumped), furnaces and the brewing stand (not dumped).
  `CrafterBlockEntity.canPlaceItem` (CrafterBlockEntity.txt 109–) refuses a disabled
  slot, a full one, and a slot when a later enabled slot holds a smaller stack of the
  same item (section 7).

### 5.3 `pushItemsTick(level, pos, state, hopper)` (146–173), the ticker

In order, every tick in the block entities' phase:

1. `cooldownTime -= 1`.
2. `tickedGameTime = level.getGameTime()`.
3. If not `isOnCooldown()` (`cooldownTime > 0`, 1041–1049): `setCooldown(0)`, then
   `tryMoveItems(level, pos, state, hopper, () -> suckInItems(level, hopper))`.

### 5.4 `tryMoveItems(level, pos, state, hopper, sucker)` (175–221)

1. Client: false. If `isOnCooldown()` or not `state.ENABLED`: false (`state` is the
   ticker's or the walk's state; **Inferred** the ticker passes the chunk's current
   state, `LevelChunk$BoundTickingBlockEntity` not read).
2. `moved = false`. If the hopper is not `isEmpty()`
   (`RandomizableContainerBlockEntity.isEmpty`, 45–52: `unpackLootTable(null)`, then
   every slot empty, BaseContainerBlockEntity.txt 135–156):
   `moved = ejectItems(level, pos, hopper)`.
3. If not `inventoryFull()` (223–248: some slot empty or below its item's max stack size):
   `moved |= sucker.getAsBoolean()` (the `|` evaluates the sucker whatever `moved` is).
4. If `moved`: `setCooldown(8)`; `setChanged(level, pos, state)`; true. Else false.

### 5.5 Sucking: `suckInItems(level, hopper)` (415–507)

`hopper` is a block hopper or a hopper cart (both `Hopper`). With `(X, Y, Z) =
(getLevelX(), getLevelY(), getLevelZ())` (block: the block's centre, 999–1027; cart: its
position with `y + 0.5`, MinecartHopper.txt 69–87):

1. `above = BlockPos.containing(X, Y + 1, Z)`, `aboveState` its state.
2. `source = getSourceContainer(level, hopper, above, aboveState)` (804–818) =
   `getContainerAt(level, above, aboveState, X, Y + 1, Z)` (5.6).
3. **With a source**: `face = DOWN`; for each `slot` of `getSlots(source, DOWN)` in order:
   if `tryTakeInItemFromSlot(hopper, source, slot, DOWN)`: return true. Return false
   (item entities are not looked at).
4. **Without**: `blocked = hopper.isGridAligned() && aboveState.isCollisionShapeFullBlock(
   level, above) && !aboveState.is(#does_not_block_hoppers)` (block hoppers are grid
   aligned, 1029–1032; carts are not). If not `blocked`: for each item entity of
   `getItemsAtAndAbove(level, hopper)` in order: if `addItem(hopper, entity)`: return
   true. Return false.

**`tryTakeInItemFromSlot(hopper, source, slot, face)`** (509–555):
`stack = source.getItem(slot)`; if not empty and `canTakeItemFromContainer(hopper,
source, stack, slot, face)` (672–697: `source.canTakeItem(hopper, slot, stack)`, default
true, and for a `WorldlyContainer` `canTakeItemThroughFace(slot, stack, face)`):
`count = stack.getCount()`; `rest = addItem(source, hopper, source.removeItem(slot, 1),
null)`; if `rest` empty: `source.setChanged()`, true; else `stack.setCount(count)` and,
if `count == 1`, `source.setItem(slot, stack)`. False.

**`getItemsAtAndAbove(level, hopper)`** (820–843): `SUCK_AABB.move(X − 0.5, Y − 0.5, Z −
0.5)` and `level.getEntitiesOfClass(ItemEntity, box, ENTITY_STILL_ALIVE)`. `SUCK_AABB`
(Hopper.txt static) = the first box of `column(16, 11, 32)`: `(0, 11/16, 0)` to
`(1, 2, 1)`. For a block hopper the box is the block's column from `y + 0.6875` to
`y + 2`; for a cart, `x − 0.5 .. x + 0.5`, `y + 0.6875 .. y + 2`, `z − 0.5 .. z + 0.5`
(its own `y` + 0.5 − 0.5). Order: `getEntitiesOfClass`'s (**Inferred** sections, then
insertion; not read).

**`addItem(container, itemEntity)`** (557–586): `copy = entity.getItem().copy()`;
`rest = addItem(null, container, copy, null)`; if `rest` empty: `entity.setItem(EMPTY)`,
`entity.discard()`, true; else `entity.setItem(rest)`, false.

### 5.6 Finding containers: `getContainerAt` (845–937), `getEntityContainer` (939–981)

- `getContainerAt(level, pos)` (845–868) = the six-argument form with the block's centre
  `(x + 0.5, y + 0.5, z + 0.5)` (ints widened, then added).
- `getContainerAt(level, pos, state, x, y, z)` (870–886): `getBlockContainer(level, pos,
  state)`, else `getEntityContainer(level, x, y, z)`.
- **`getBlockContainer`** (888–937): a block that is a `WorldlyContainerHolder` (the
  composter) gives `getContainer(state, level, pos)`; else, if `state.hasBlockEntity()`
  and the block entity is a `Container`: if it is a `ChestBlockEntity` and the block a
  `ChestBlock`, `ChestBlock.getContainer(block, state, level, pos, true)` (the double
  chest's compound, **ignoring what blocks it**: a cat or a block above does not stop a
  hopper; section 3), else the block entity. Else null.
  Block entities that are `Container`s in 26.3 (**Inferred** for the classes not
  dumped): chest and trapped chest (and copper chests, which use `CHEST`), barrel,
  dispenser, dropper, hopper, shulker boxes, crafter (all read); furnace, blast furnace,
  smoker, brewing stand, chiseled bookshelf, decorated pot, jukebox, shelf (not read).
  The lectern and the ender chest are not.
- **`getEntityContainer(level, x, y, z)`** (939–981): `list = level.getEntities(null,
  AABB(x − 0.5, y − 0.5, z − 0.5, x + 0.5, y + 0.5, z + 0.5), CONTAINER_ENTITY_SELECTOR)`;
  empty: null; else `list.get(level.getRandom().nextInt(list.size()))`.
  `CONTAINER_ENTITY_SELECTOR` is `e instanceof Container && e.isAlive()`
  (EntitySelector.txt `lambda$static$2`, 206–218; **Inferred** which lambda belongs to
  which field, by the order of the static initialiser, 245–272). Among N5's entities:
  `MinecartChest`, `MinecartHopper`, `ChestBoat`, `ChestRaft`. The box is tested by
  intersection (strict, **Inferred** `AABB.intersects`).

### 5.7 Ejecting: `ejectItems(level, pos, hopper)` (250–317) and `addItem` (588–792)

1. `target = getAttachedContainer(level, pos, hopper)` (794–802) =
   `getContainerAt(level, pos.relative(facing))` (5.6). None: false.
2. `face = facing.getOpposite()`. If `isFullContainer(target, face)` (379–413: every slot
   of `getSlots(target, face)` has `count >= its stack's max stack size`; an empty slot's
   count 0 is below): false.
3. For `slot` in `0..5`: skip an empty one; `count = stack.getCount()`; `rest =
   addItem(hopper, target, hopper.removeItem(slot, 1), face)`; if `rest` empty:
   `target.setChanged()`, true; else `stack.setCount(count)` and, if `count == 1`,
   `hopper.setItem(slot, stack)`. After the loop: false. One item per move.

**`addItem(source, target, stack, face)`** (588–644): if `target` is a `WorldlyContainer`
and `face` is not null: for each slot of `getSlotsForFace(face)` while `stack` is not
empty, `stack = tryMoveInItem(source, target, stack, slot, face)`; else for every slot
`0..size` in order while not empty, the same. Returns what is left. (Sucking passes
`face = null` into the hopper, so its five slots in order.)

**`tryMoveInItem(source, target, stack, slot, face)`** (699–792):

1. `here = target.getItem(slot)`. Unless `canPlaceItemInContainer(target, stack, slot,
   face)` (646–670: `target.canPlaceItem(slot, stack)`, default true, and for a
   `WorldlyContainer` `canPlaceItemThroughFace(slot, stack, face)`): return `stack`.
2. `moved = false`; `wasEmpty = target.isEmpty()` (the whole target, before).
3. If `here` is empty: `target.setItem(slot, stack)`; `stack = EMPTY`; `moved = true`.
   Else if `canMergeItems(here, stack)` (983–997: `here.count <= here.maxStackSize` and
   `isSameItemSameComponents(here, stack)`): `room = stack.getMaxStackSize() −
   here.getCount()`; `n = min(stack.getCount(), room)`; `stack.shrink(n)`;
   `here.grow(n)`; `moved = n > 0`.
4. If `moved`: if `wasEmpty` and `target` is a `HopperBlockEntity` that is not
   `isOnCustomCooldown()` (`cooldownTime > 8`, 1051–1060): `k = 0`; if `source` is a
   `HopperBlockEntity` and `target.tickedGameTime >= source.tickedGameTime`: `k = 1`;
   `target.setCooldown(8 − k)`. Then `target.setChanged()`.
5. Return `stack`.

The cooldown handed on applies to a hopper pushing into a hopper (the `source` is the
pushing hopper). When a hopper sucks from a hopper above, the target is the sucking
hopper itself (with `source` the hopper above), which `tryMoveItems` then sets to 8
anyway; from an item entity or a cart, `source` is null or not a block hopper (`k = 0`).

### 5.8 `HopperBlockEntity.entityInside(level, pos, state, entity, hopper)` (1075–1115)

If `entity` is an `ItemEntity` with a non-empty stack and `entity.getBoundingBox().move(
−x, −y, −z)` (the block's ints negated, then widened) intersects `getSuckAabb()` (the
unmoved `SUCK_AABB`): `tryMoveItems(level, pos, state, hopper, () -> addItem(hopper,
entity))` (lambda 1127–1132). No cooldown decrement, no `tickedGameTime` stamp: those are
the ticker's. The walk runs in the item's tick (`applyEffectsFromBlocks`, after its move),
so the item enters the hopper in the entities' phase; Clustine's phases are block events,
entities, block entities (ADR-0026, "Found while building"), the same order.

### 5.9 `MinecartHopper` (MinecartHopper.txt)

- `extends AbstractMinecartContainer implements Hopper`; 5 slots (34–37); display block
  a hopper, offset 1 (23–32); drops `HOPPER_MINECART` (184–187); menu `HopperMenu`
  (222–230, section 3).
- `enabled` (default true, 9–21); saved as `Enabled` (`putBoolean`, 197–207), read with
  default true (209–220). Not in its synched data.
- **`activateMinecart(level, x, y, z, powered)`** (39–54): `enabled = !powered` if it
  differs. Called every tick the cart is on an activator rail (N5 brief 3.7; the repo's
  `activate_minecart` in `minecarts.rs` leaves it out today).
- **`tick()`** (94–103): `consumedItemThisFrame = false`; `super.tick()` (the cart's whole
  tick, its move included); `tryConsumeItems()`.
- `makeStepAlongTrack` (105–116) also calls `tryConsumeItems()` after each step: only
  `NewMinecartBehavior` calls it, so not under the old behaviour Clustine runs.
- **`tryConsumeItems()`** (118–141): server, alive, `enabled`, and not
  `consumedItemThisFrame`: if `suckInItems()`: `consumedItemThisFrame = true`,
  `setChanged()` (a no-op for carts).
- **`suckInItems()`** (143–182): `HopperBlockEntity.suckInItems(level, this)` (5.5, with
  `X, Y, Z = x, y + 0.5, z`, not grid aligned, so a solid block above never stops it; the
  container above is the block at `containing(x, y + 1.5, z)`, or a container entity in
  the box centred there); if that took nothing: every `ItemEntity` in
  `getBoundingBox().inflate(0.25, 0, 0.25)` (`ENTITY_STILL_ALIVE`), in order,
  `HopperBlockEntity.addItem(this, entity)`, the first true returns true.
- No cooldown and no ejecting: what a hopper cart holds leaves only through a hopper
  that pulls from it (the cart in the box above the hopper's block) or by its drop.
- What it needs in Clustine: an `enabled` field in `EntityData::Minecart` (durable, and
  carried by lends, splits and crossings), the activator rail's call, the suck after the
  cart's tick, with the item search over the region's entities, and the cart's
  `contents` written as an entity change.

### 5.10 What exists and what changes in Clustine

- Nothing of the hopper exists beyond its block class (`BlockClass::HopperBlock`, its
  block entity type 18, `clustine_blocks::signal` counting it among blocks that read
  signals). `ENABLED` is not updated today.
- Needed: a typed block entity `Hopper { items: [ItemStack; 5], cooldown: i32,
  ticked: u64 }` (`ticked` in memory only, 0 after a load, as the game's), durable as
  the game saves it (`Items`, `TransferCooldown`, an optional `LootTable`), a ticker in
  the list of `block_entities.rs`, and container access for the other block entities
  (section 3's typed containers): chest (with its double), barrel, dispenser, dropper,
  shulker box, crafter, and a fallback that leaves an unported container (furnace…)
  alone rather than mis-filling it.
- The hopper's moves change block entities in two chunks at once (its own and its
  target's or source's) and, for an item entity, an entity: all in one commit.
- `Region::analog_output_at` must read a hopper's (and every container's) fill from its
  typed block entity, and `setChanged` must reach `update_neighbour_for_output_signal`.

## 6. Pressure plates and tripwire with entities

Classes read: `BasePressurePlateBlock` (BasePressurePlateBlock.txt, 314 lines),
`PressurePlateBlock` and `$1` (104 and 30), `WeightedPressurePlateBlock` (103),
`TripWireBlock` and `$1` (706, 60), `TripWireHookBlock` (740), `Entity`
(`isIgnoringBlockTriggers`, `checkInsideBlocks`, `applyEffectsFromBlocks`),
`EntitySelector`, all in `n6-javap/`; `EntityGetter`, `ArmorStand`, `DetectorRailBlock`,
`AbstractMinecartContainer` in `n5-javap/`. What R3b built for players is in
`crates/clustine-sim/src/region/pressed.rs`; detector rails in `rails.rs` and
`minecarts.rs`.

### 6.0 Findings of this section

1. **What counts on a plate is every entity but spectators and markers.** A plate's
   power is recounted by `getEntitiesOfClass(cls, TOUCH_AABB.move(pos), NO_SPECTATORS
   .and(e -> !e.isIgnoringBlockTriggers()))`. `cls` is `Entity` for a plate whose block
   set is `EVERYTHING` and for both weighted plates, `LivingEntity` for one that is
   `MOBS`. `isIgnoringBlockTriggers` is false for every entity but an armour stand that
   is a marker (ArmorStand.txt 1197–1201). So items, minecarts, boats, falling blocks,
   primed TNT, armour stands and players press wooden and weighted plates; only players,
   armour stands (and N7's mobs) press stone ones. Hanging entities and cushions count
   too where their boxes meet the touch box, although they never walk into blocks
   themselves (6.2).
2. **A weighted plate counts entities, not items**: each item entity is one, whatever its
   stack. `power = ceil(min(n, max) / (float) max × 15f)` with `max` 15 for the light
   (gold) plate and 150 for the heavy (iron); n = 1 gives 1 on each (6.3).
3. **Which plates are `MOBS` is in `BlockSetType`, not read.** **Inferred** (from the
   game's registrations, `Blocks` and `BlockSetType` not dumped): stone and polished
   blackstone plates are `MOBS`; every wooden plate (oak to cherry, bamboo, crimson,
   warped, pale oak, mangrove) is `EVERYTHING`. Clustine's plates treat all alike, as
   only players pressed them so far.
4. **Tripwire is pressed by the entity that walked in alone, and kept by all of them.**
   `entityInside` calls `checkPressed(level, pos, List.of(entity))`: the entity itself,
   if it does not ignore triggers. The scheduled re-check (every 10 ticks while powered)
   uses `level.getEntities(null, shape bounds)`, which is `NO_SPECTATORS` (EntityGetter
   .txt 19–26), and any entity not ignoring triggers keeps it pressed. Tripwire's
   inside shape is its outline shape (`getEntityInsideCollisionShape` = `getShape`), so
   the walk's swept test (`collidedWithShapeMovingFrom`) decides, not a plain overlap
   (6.4).
5. **Every entity's block walk must reach the region's rules.** The game calls
   `entityInside` from each entity's own tick (`applyEffectsFromBlocks`, after its move),
   so items, carts, boats, falling blocks, primed TNT and armour stands press plates,
   tripwire and detector rails at their place in the entity order. In Clustine only
   players (`players_touch_blocks`, at the end of the phase) and carts (`CartInside`)
   reach them; the items', boats', falling blocks' and stands' walks (`physics/item.rs`,
   `boats.rs`, `falling.rs`, `living/inside.rs`) must return the plates, tripwire,
   detector rails and hoppers they met, with the step, for the region to run.
6. **The count reads every entity in the touch box, and the moment matters.** A plate
   pressed by an item in that item's tick counts the entities where they are at that
   moment: those that ticked already at their new places, the rest at their old. A
   port that applies the walks after all entities have moved counts differently in
   one tick only when two entities enter or leave together; the plan should say which.
7. **Detector rails** (built in N5): what remains is that any entity, not only a cart,
   entering an unpowered detector rail runs `checkPressed`, which finds no cart and
   changes nothing but tells the comparators beside it (`updateNeighbourForOutputSignal`
   runs on every check). Players and other entities do not reach it in Clustine. And a
   comparator reading a powered detector rail reads the cart's fill only when the rail
   checks (every 20 ticks while powered, `DetectorRailBlock.tick`): a hopper or a menu
   changing a cart's contents tells no comparator (`AbstractMinecartContainer.
   setChanged` does nothing, AbstractMinecartContainer.txt 72–74). That is the game's;
   keep it.
8. **Shears disarm tripwire** (`playerWillDestroy`, 209–246), which Clustine does not do:
   a player breaking tripwire with shears in the main hand first sets `DISARMED = true`
   with flags 260 (`UPDATE_INVISIBLE | UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS`, **Inferred**
   the flag names) and emits `GameEvent.SHEAR`, then breaks it as usual: the hooks
   recalculate with a disarmed wire and do not click on. In creative too
   (`playerWillDestroy` runs for every break, **Inferred** from `ServerPlayerGameMode`,
   N5 brief).
9. **Game events.** A plate's change emits `BLOCK_ACTIVATE` or `BLOCK_DEACTIVATE` with the
   entity that pressed it (null from its scheduled tick); the hooks' `emitState` emits
   `BLOCK_ACTIVATE`, `BLOCK_DEACTIVATE`, `BLOCK_ATTACH`, `BLOCK_DETACH` (null entity);
   tripwire shorn emits `SHEAR` with the player. They matter only to sculk sensors
   (section 8).
10. The only random draw here is a hook's detach sound's pitch, `1.2f / (level.random
    .nextFloat() × 0.2f + 0.9f)` (TripWireHookBlock.txt 538–551, already built). Plates,
    tripwire and detector rails draw nothing.

### 6.1 `BasePressurePlateBlock` (BasePressurePlateBlock.txt)

- **Shapes** (static, 293–313; 1/16 units): `SHAPE_PRESSED = column(14, 0, 0.5)`,
  `SHAPE = column(14, 0, 1)`, `TOUCH_AABB` = the box of `column(14, 0, 4)`: `(1/16, 0,
  1/16)` to `(15/16, 4/16, 15/16)`. `getShape` (24–33): pressed (`getSignalForState > 0`)
  or not. The inside shape is not overridden (the full block), so `entityInside` runs for
  any entity whose deflated box is in the plate's block.
- **`getPressedTime()`** (35–38): 20 (weighted plates: 10).
- **`updateShape`** (45–68): from below and unable to survive: air. **`canSurvive`**
  (70–87): `canSupportRigidBlock(below)` or `canSupportCenter(below, UP)`.
- **`tick(state, level, pos, random)`** (89–104): `s = getSignalForState(state)`; if
  `s > 0`: `checkPressed(null, level, pos, state, s)`. No draw.
- **`entityInside(state, level, pos, entity, …)`** (106–125): server only; `s =
  getSignalForState(state)`; if `s == 0`: `checkPressed(entity, level, pos, state, s)`.
  (While powered, entities walking in do nothing; the scheduled tick recounts.)
- **`checkPressed(entity, level, pos, state, old)`** (127–212):
  1. `new = getSignalStrength(level, pos)`; `wasOn = old > 0`; `on = new > 0`.
  2. If `old != new`: `changed = setSignalForState(state, new)`; `level.setBlock(pos,
     changed, 2)`; `updateNeighbours(level, pos)`; `level.setBlocksDirty(pos, state,
     changed)` (render only).
  3. If `!on && wasOn`: `playSound(null, pos, type.pressurePlateClickOff(), BLOCKS)`
     (volume 1, pitch 1) and `gameEvent(entity, BLOCK_DEACTIVATE, pos)`; else if `on &&
     !wasOn`: `pressurePlateClickOn()` and `BLOCK_ACTIVATE`.
  4. If `on`: `scheduleTick(pos, this, getPressedTime())`.
- **`affectNeighborsAfterRemoval`** (214–226): unless moved by a piston, if it was
  powered, `updateNeighbours`.
- **`updateNeighbours(level, pos)`** (228–239): `updateNeighborsAt(pos, this)`, then
  `updateNeighborsAt(pos.below(), this)`.
- **Signals**: `isSignalSource` true (253–256); `ownSignal` = `getSignalForState`
  (258–263); `getDirectSignal(…, dir)` = that when `dir == UP`, else 0 (241–251).
- **`getEntityCount(level, box, cls)`** (265–275, lambda 283–291): `level.
  getEntitiesOfClass(cls, box, NO_SPECTATORS.and(e -> !e.isIgnoringBlockTriggers()))
  .size()`.

All of this but the entity count is in `pressed.rs` (`check_plate`, `plate_strength`,
`plate_updates_neighbours`, the sounds, the delays 20 and 10).

### 6.2 `PressurePlateBlock` (PressurePlateBlock.txt, `$1`)

- `POWERED` (default false); `getSignalForState` = 15 or 0 (24–35); `setSignalForState`
  = `POWERED = s > 0` (37–49).
- **`getSignalStrength(level, pos)`** (51–84): by `type.pressurePlateSensitivity()`
  (`$1`'s switch map: `EVERYTHING` → 1, `MOBS` → 2; anything else throws
  `MatchException`): `cls = Entity` for `EVERYTHING`, `LivingEntity` for `MOBS`;
  `getEntityCount(level, TOUCH_AABB.move(pos), cls) > 0 ? 15 : 0`.
- Entities and what they press (`EVERYTHING` / `MOBS`), with the rule that the box must
  meet the touch box strictly (**Inferred** `AABB.intersects` is strict, as the repo's
  `overlaps`):

  | Entity in Clustine | wooden, gold, iron | stone, polished blackstone |
  |---|---|---|
  | player (not spectator) | yes | yes |
  | item | yes | no |
  | minecart (any kind), boat, raft | yes | no |
  | falling block | yes (as it passes) | no |
  | primed TNT (N6) | yes | no |
  | armour stand | yes, unless a marker | yes, unless a marker |
  | item frame, painting, leash knot, cushion | yes where its box meets (they never walk
  into blocks, so only a recount sees them) | no |
  | experience orb, projectiles (N9) | yes | no |

### 6.3 `WeightedPressurePlateBlock` (WeightedPressurePlateBlock.txt)

- `POWER` 0–15 (default 0); `maxWeight` from the constructor (7–27): **Inferred** 15
  (light, gold) and 150 (heavy, iron), from `Blocks`, not read (the repo's
  `plate_weight` has them).
- **`getSignalStrength(level, pos)`** (29–58): `n = min(getEntityCount(level,
  TOUCH_AABB.move(pos), Entity), maxWeight)`; if `n > 0`: `f = (float) min(maxWeight, n)
  / (float) maxWeight`; return `Mth.ceil(f × 15f)` (float multiply, `Mth.ceil(float)`:
  the int part, plus one if above it); else 0.
- `getSignalForState` = `POWER` (61–68), `setSignalForState` sets it (70–78);
  `getPressedTime` = 10 (80–83).

The repo's `plate_strength` has the formula; only the count changes, to all entities.

### 6.4 `TripWireBlock` (TripWireBlock.txt)

- **Shapes** (static, 690–705): `SHAPE_ATTACHED = column(16, 1, 2.5)`,
  `SHAPE_NOT_ATTACHED = column(16, 0, 8)`; `getShape` by `ATTACHED` (78–90).
  `getEntityInsideCollisionShape` (317–323) = `getShape`: the walk tests the swept box
  against it (N4 brief, `checkInsideBlocks`).
- **`entityInside(state, level, pos, entity, …)`** (325–350): server only; if `POWERED`,
  or the tick queue has a tick of this block at `pos` (`hasScheduledTick`): nothing;
  else `checkPressed(level, pos, List.of(entity))`.
- **`tick`** (352–367): if the block at `pos` is `POWERED`: `checkPressed(level, pos)`.
- **`checkPressed(level, pos)`** (369–391): `list = level.getEntities(null,
  state.getShape(level, pos).bounds().move(pos))` (`NO_SPECTATORS`), then the
  three-argument form.
- **`checkPressed(level, pos, entities)`** (393–463): `was = POWERED`; `on` = some entity
  of the list with `!isIgnoringBlockTriggers()`; if `on != was`: `setBlockAndUpdate(pos,
  state with POWERED = on)` and `updateSource(level, pos, new)`; if `on`:
  `scheduleTick(pos, this, 10)`; else if `was`: `scheduleTick(pos, this, 1)`.
- **`updateSource`** (247–316): the hooks back along the line (built:
  `tripwire_update_source`).
- **`playerWillDestroy`** (209–246): see 6.0 item 8.

`pressed.rs` has all of it but the entity list: `entity_inside` for players only
(final-box overlap rather than the sweep), `on_tripwire` counting players only.

### 6.5 `TripWireHookBlock` (TripWireHookBlock.txt), for reference

Built in R3b (`hook_calculate_state`, `hook_notifies_neighbours`, `hook_sounds`):
`calculateState` (189–468) walks up to 41 positions (`bipush 42`, 234–240) for the hook
facing back; a powered, non-disarmed tripwire on the line powers both hooks;
`scheduleTick(pos, this, 10)` (315–316) when attached and powered; `emitState` (481–563)
plays `TRIPWIRE_CLICK_ON` (0.4, 0.6), `TRIPWIRE_CLICK_OFF` (0.4, 0.5), `TRIPWIRE_ATTACH`
(0.4, 0.7), `TRIPWIRE_DETACH` (0.4, `1.2f / (nextFloat() × 0.2f + 0.9f)`, a draw) with their
game events; `notifyNeighbors` (564–587). Nothing in the hook depends on entities.

### 6.6 What changes in Clustine

- **The entity's walk**: each entity kind's `Inside::met` with `block = true` hands the
  position, the state and the step back, and the region runs `entityInside` of plates
  (unpowered: `check_plate`), tripwire (unpowered, no tick queued: press if this entity
  does not ignore triggers), detector rails (unpowered: `detector_rail_check`) and
  hoppers (5.8), in the walk's order, during that entity's tick. Players keep their
  phase.
- **The counts**: `plate_strength` and `on_tripwire` count all entities of the region's
  loaded chunks (and of copies, 11) whose boxes meet the box, skipping marker armour
  stands, with `LivingEntity` only (players and armour stands until N7) for `MOBS`
  plates. The box test stays strict.
- **Block set data**: a plate's sensitivity (`MOBS` or `EVERYTHING`) from the block, in
  `clustine-data` (by block name until `BlockSetType` is read).
- **Shears**: `DISARMED` set before a tripwire is broken by a player holding shears.

## 7. The crafter (optional for N6)

Read: `CrafterBlock` (CrafterBlock.txt, 611 lines) and `CrafterBlock$1` (its switch over
`Direction`), `CrafterBlockEntity` (552) and `CrafterBlockEntity$1` (its
`ContainerData`), `CrafterMenu` (its slots, `setSlotState`, `quickMoveStack`,
`refreshRecipeResult`), `ServerGamePacketListenerImpl.handleContainerSlotStateChanged`
(ServerGamePacketListenerImpl.txt 2824–2861) and `HopperBlockEntity.getContainerAt`/
`addItem` (4.18). Not dumped: `RecipeCache`, `CraftingInput`, the recipe classes,
`CrafterSlot`, `NonInteractiveResultSlot`, `ResultContainer`, `ItemStack.onCraftedBySystem`.

### 7.0 Findings of this section

1. **The crafter fires on a rising edge of `hasNeighborSignal(pos)` only**: no
   quasi-connectivity (CrafterBlock.txt 67–133, against the dispenser's `pos.above()`).
   It sets `triggered` (flags 2) **and the block entity's `triggered` data slot** (shown
   in the menu as the powered arrow), and schedules a tick in 4. Falling: `triggered`
   and `crafting` both false (flags 2) and the entity's slot 0.
2. **Placed already powered, it crafts**: `getStateForPlacement` (195–247) sets
   `triggered = hasNeighborSignal(clickedPos)`, and `setPlacedBy` (248–262) schedules the
   tick in 4 when it is. (The dispenser does neither.)
3. **A craft needs the recipe book of the server: every `crafting` recipe** (shaped,
   shapeless, transmute, the special ones: 1 266 of the 1 764 files in
   `generated/data/minecraft/recipe/`), matched against the 3 × 3 grid as
   `CraftingInput` (trimmed to its occupied rectangle, **Inferred**: `CraftingInput` not
   read), through a 10-entry `RecipeCache` (not read; a cache only, the result is
   `RecipeManager`'s first match, **Inferred**).
4. **The result and every remainder go into the container in front, else out as items,
   one call each**; every grid stack then shrinks by one. The success sound (1049) and the
   white smoke (2010) are made **only when something was dropped into the world**, once
   per dropped stack; a craft that fills a chest in front is silent (7.2).
5. **A failed craft (no recipe, or an empty result) is level event 1050** and changes
   nothing; the crafter keeps `crafting` false.
6. **`crafting` is lit for six ticks** by the block entity's ticker (`serverTick`,
   CrafterBlockEntity.txt 430–456): 6 → 0, and at 0 `crafting = false` with
   `setBlockAndUpdate` (flags 3).
7. **Hoppers fill a crafter evenly**: `canPlaceItem(slot, stack)` refuses a disabled
   slot, a full slot, and a non-empty slot when any later enabled slot is empty or holds
   fewer of the same item (7.3).
8. **The comparator counts slots**, not fullness: the number of slots that hold an item
   or are disabled (`getRedstoneSignal`, 464–490), 0 to 9.
9. **The menu has its own serverbound packet**, `container_slot_state_changed` (container
   id, slot id, new state), which toggles a slot's disabled state when it is empty (7.4).
10. **What N6 needs to build it**: the dispenser's power rule (section 4), containers with
    menus (section 3), the container search and `addItem` (sections 4.18, 5), and a
    recipe matcher over the generated recipes (shaped with mirroring, shapeless,
    transmute, the 11 other types (banner duplication, dyeing, firework rocket and stars,
    map
    extending, book cloning, decorated pot, repair, imbuing, shield decoration), and the
    remainders: buckets from cake's milk, bottles
    from honey, the empty `crafting_remainder` of the item's components). The matcher is
    the large part and is the same the player's crafting table and inventory grid will
    use; the crafter is optional for N6 (the plan's line), and a creative player meets it
    rarely. Recommendation: **leave the crafter to the step that brings crafting**; until
    then a crafter is a container with a menu (7.4) that fails with 1050 when powered.

### 7.1 Block and state (CrafterBlock.txt)

- `ORIENTATION` (`FrontAndTop`, twelve values: a front and a top), `TRIGGERED`,
  `CRAFTING` (`createBlockStateDefinition` 565–585; static 597–610). `MAX_CRAFTING_TICKS
  = 6`, `CRAFTING_TICK_DELAY = 4`, `CRAFTER_ADVANCEMENT_DIAMETER = 17`; `RECIPE_CACHE =
  new RecipeCache(10)`.
- `getStateForPlacement` (195–247): `front = getNearestLookingDirection().getOpposite()`;
  `top` by `front` (`CrafterBlock$1` maps `DOWN` 1, `UP` 2, `NORTH` 3, `SOUTH` 4, `WEST`
  5, `EAST` 6): front down → `getHorizontalDirection().getOpposite()`; front up →
  `getHorizontalDirection()`; horizontal → `UP`. `ORIENTATION =
  FrontAndTop.fromFrontAndTop(front, top)`, `TRIGGERED = level.hasNeighborSignal(
  clickedPos)`, `CRAFTING` false (default). `rotate`/`mirror` by `OctahedralGroup`.
- `useWithoutItem` (271–292): opens the block entity's menu (`SUCCESS`; no statistic).
- `affectNeighborsAfterRemoval` (263–270): `Containers.updateNeighboursAfterDestroy`.
- Comparator: `hasAnalogOutputSignal` true; `getAnalogOutputSignal` (49–66) =
  `be.getRedstoneSignal()` (7.0 item 8), 0 when the block entity is missing.
- `newBlockEntity` (169–194): a `CrafterBlockEntity` whose `triggered` data is set from
  the state's `TRIGGERED`.
- `getTicker` (143–155): `CrafterBlockEntity::serverTick` on the server.

### 7.2 The tick: `dispenseFrom(state, level, pos)` (293–400) and `dispenseItem` (409–534)

```
be = level.getBlockEntity(pos) as CrafterBlockEntity, or return
input = be.asCraftInput()
recipe = RECIPE_CACHE.get(level, input)                   // getPotentialResults, 401-408
if recipe.isEmpty(): levelEvent(1050, pos, 0); return     // SOUND_CRAFTER_FAIL
result = recipe.value().assemble(input)
if result.isEmpty(): levelEvent(1050, pos, 0); return
be.setCraftingTicksRemaining(6)
level.setBlock(pos, state.setValue(CRAFTING, true), 2)    // the tick's state, with its
TRIGGERED
result.onCraftedBySystem(level)
dispenseItem(level, pos, be, result, state, recipe)
for rem in recipe.value().getRemainingItems(input):        // in grid order
    if !rem.isEmpty(): dispenseItem(level, pos, be, rem, state, recipe)
for s in be.getItems(): if !s.isEmpty(): s.shrink(1)       // lambda 586-596
be.setChanged()
```

`dispenseItem(level, pos, be, stack, state, recipe)`:

```
front = state.ORIENTATION.front()
into = HopperBlockEntity.getContainerAt(level, pos + front)   // may draw nextInt (4.18)
rest = stack.copy()
if into != null and (into is CrafterBlockEntity or stack.count >
into.getMaxStackSize(stack)):
    while !rest.isEmpty():                                    // one at a time
        left = HopperBlockEntity.addItem(be, into, rest.copyWithCount(1), front.opposite)
        if !left.isEmpty(): break
        rest.shrink(1)
elif into != null:
    while !rest.isEmpty():                                    // the whole stack, until it stops moving
        n = rest.count
        rest = HopperBlockEntity.addItem(be, into, rest, front.opposite)
        if n == rest.count: break
if !rest.isEmpty():
    c = Vec3.atCenterOf(pos); p = c.relative(front, 0.7)
    DefaultDispenseItemBehavior.spawnItem(level, rest, 6, front, p)   // 4.3: level draws, the item's own
    for each ServerPlayer in level within AABB.ofSize(c, 17, 17, 17):
        CRAFTER_RECIPE_CRAFTED.trigger(player, recipe.id(), be.getItems())
    levelEvent(1049, pos, 0)                                  // SOUND_CRAFTER_CRAFT
    levelEvent(2010, pos, front.get3DDataValue())             // PARTICLES_SHOOT_WHITE_SMOKE
```

One at a time into another crafter (so its `canPlaceItem` spreads them) or where the stack
is larger than the receiver's maximum; otherwise whole. `Vec3.relative(dir, d)` is
**Inferred** `add(d · step)` (Vec3 read in N5; `relative` not checked). The advancement
trigger is not Clustine's (no advancements).

### 7.3 The block entity (CrafterBlockEntity.txt)

- `extends RandomizableContainerBlockEntity implements CraftingContainer`; `items` (9),
  `craftingTicksRemaining` (0), `containerData` = `CrafterBlockEntity$1`: ten ints,
  0–8 the slot states (`SLOT_ENABLED = 0`, `SLOT_DISABLED = 1`), 9 `triggered`
  (`DATA_TRIGGERED`, `NUM_DATA = 10`) (constructor 29–51; `$1` 25–58).
- `setSlotState(slot, enabled)` (69–88): only if `slotCanBeDisabled(slot)` (491–510:
  `-1 < slot < 9` and the slot is **empty**); sets 0 for enabled, 1 for disabled;
  `setChanged`. `isSlotDisabled(slot)` (89–108): `0 ≤ slot < 9` and state 1.
- `setItem(slot, stack)` (300–315): **a disabled slot is enabled first**, then the base's
  `setItem`.
- `canPlaceItem(slot, stack)` (109–149): disabled → false; `existing = items[slot]`;
  `existing.count ≥ stack.getMaxStackSize()` → false; existing empty → true; else
  `!smallerStackExist(existing.count, existing, slot)`. `smallerStackExist(count,
  existing, slot)` (150–184): for `j` in `slot + 1 .. 8`, not disabled: an **empty** `j`
  → true; or `items[j].count < count` and same item and components → true. So an item
  goes to the first slot that is empty, or holds the least of it, in order.
- `getRedstoneSignal` (464–490): count of `i` in 0..8 with an item, or empty and
  disabled.
- `serverTick` (430–456): `t = craftingTicksRemaining − 1`; `t < 0` → return; store `t`;
  `t == 0` → `setBlockAndUpdate(pos, state.setValue(CRAFTING, false))`.
- `setTriggered(b)` / `isTriggered()` (404–429): data slot 9 as 1 or 0.
- Saved (`saveAdditional`, 238–263): the base's, `crafting_ticks_remaining` (int), `Items`
  (or the loot table), `disabled_slots` (an int array of the disabled indices, in
  order, 365–392), `triggered` (int, 393–403). Loaded (`loadAdditional`, 185–237): ticks
  (default 0), items, all slots enabled, then `disabled_slots` applied through
  `slotCanBeDisabled` (so a disabled slot that holds an item loads enabled), `triggered`
  (default 0). `getWidth`/`getHeight` 3; `fillStackedContents` for the recipe book.
- Name `container.crafter`.

### 7.4 The menu (`CrafterMenu`) and its packet

- `MenuType.CRAFTER_3x3`; slots 0–8 are `CrafterSlot`s over the grid (x 26 + 18 · col, y
  17 + 18 · row), 9–44 the player's (`addStandardInventorySlots(inventory, 8, 84)`), 45 a
  `NonInteractiveResultSlot` showing the result (134, 35); data slots: the ten of
  `containerData` (`addDataSlots`) (55–156). The menu listens to its own slots and
  refreshes the shown result on each change (`slotChanged` → `refreshRecipeResult`,
  285–319, 326–331: the same `getPotentialResults` and `assemble`, empty when none).
- `quickMoveStack` (212–276): from the grid (index < 9) to 9..45 backwards; from the
  inventory into 0..9 forwards (`moveItemStackTo`, section 3; it respects `mayPlace` of the
  `CrafterSlot`, **Inferred** `!menu.isSlotDisabled` and the container's `canPlaceItem`).
- `handleContainerSlotStateChanged(packet)` (ServerGamePacketListenerImpl.txt 2824–2861):
  not a spectator, `packet.containerId == player.containerMenu.containerId`, the menu is a
  `CrafterMenu` whose container is a `CrafterBlockEntity`: `be.setSlotState(packet.slotId,
  packet.newState)`. The change reaches the client as a `container_set_data` from the
  menu's `broadcastChanges` (section 3). The packet's codec is not dumped (section 12).
- `CrafterMenu.setSlotState` (157–177) is the client's path (it calls
  `broadcastChanges` on its own menu); `isPowered()` reads data slot 9.

### 7.5 Draws

A craft draws only where something is dropped: per dropped stack, `getContainerAt`'s
possible `nextInt` (container entities in front, when no block container) and then
`spawnItem`'s four level draws (4.3) and the item's own six. A craft that fills a
container draws only the possible `nextInt` per `dispenseItem` call. Recipe matching
draws nothing (**Inferred**; special recipes such as the firework star read no random).

## 8. Sculk sensors and vibrations

Read: `SculkSensorBlock` (780 lines), `CalibratedSculkSensorBlock` (132),
`SculkSensorBlockEntity` (135) and its `$VibrationUser` (128), `VibrationSystem` (416)
and its `$Data` (156), `$Listener` (243), `$Ticker` (300), `$User` (95), the game-event
calls of `Entity` (n6 dump) and of the n5 dump (`Level`, `ServerLevel`, `LivingEntity`,
`Player`, `Mob`, the N5 entities and items), `RedStoneOreBlock` (291), every N6 class
that posts a game event (listed in 8.1), the tags `data/minecraft/tags/game_event/*` and
`tags/block/{occludes_vibration_signals, dampens_vibrations, vibration_resonators,
wool}`, and `reports/registries.json` (`minecraft:game_event`, `particle_type`,
`position_source_type`). Not dumped and needed: `GameEvent` (the registry and each
event's notification radius), `GameEventDispatcher`, `GameEventListener` (its
`DeliveryMode`), `EuclideanGameEventListenerRegistry`, `VibrationSelector`,
`VibrationInfo`, `BlockPositionSource`, `EntityPositionSource`,
`VibrationParticleOption`, `CalibratedSculkSensorBlockEntity` (and its user),
`SculkShriekerBlock(Entity)`, `LevelAccessor`/`LevelWriter` (the `gameEvent` overloads),
`LevelChunk` (how listeners are registered), `ClientboundLevelParticlesPacket`, and the
blocks already built that post events (levers, buttons, doors, pistons, note blocks,
bells, `Block.playerWillDestroy`, `BlockItem.place`). What those say is marked
**Inferred** below, from the 1.20–1.21 sources' shape, and listed in section 12.

### 8.0 Findings of this section

1. **Nothing of game events exists in Clustine.** `grep -ri "game_event\|gameevent"`
   finds only `clustine_protocol::packets::play::GameEvent`, which is the unrelated
   clientbound `game_event` packet (the "change game state" packet: rain, game mode,
   …). N6 has to build the whole chain: a post from every place the game posts one
   (8.1), the dispatch to listeners within the event's radius (8.2), the listener's
   test (8.3–8.5), the selection a tick later (8.6), the travel, the receipt and the
   sensor's block (8.8). A name other than `GameEvent` is needed in the simulation to
   keep the two apart (`Vibration`/`WorldEvent`, say).
2. **Most of what a sensor hears comes from code already built.** Of the 26.3 jar's
   posts, N4 and N5 code already runs the moves and actions that post `step`, `swim`,
   `hit_ground`, `splash`, `bounce`, `entity_mount`, `entity_dismount`,
   `entity_place` (every N5 item), `entity_damage` (armour stands, vehicles),
   `entity_die`, `entity_interact`, `entity_action`, `shear`, `block_attach` (knots,
   leads), `block_change` (item frames), `container_open`/`container_close` (chest
   carts and boats), `equip`/`unequip` (armour stands) and `block_destroy`
   (`Level.destroyBlock`); and the redstone of ADR-0026 posts `block_activate`,
   `block_deactivate`, `block_open`, `block_close`, `note_block_play`,
   `block_change` (**Inferred**: those blocks' classes are not dumped). The N4 brief
   marked every one of these "skip" (n4-brief 1.0 item 14, 342, 456, 656, 918, 1258);
   N6 has to go back and post them. Breaking and placing blocks by hand post
   `block_destroy` and `block_place` (**Inferred**: `Block.playerWillDestroy`,
   `BlockItem.place`).
3. **A sensor hears a player's steps only when the server moves the player.** A
   player's step posts `step` from `Entity.move` (`applyMovementEmissionAndPlaySound`,
   Entity.txt 2032–2142, `vibrationAndSoundEffectsFromBlock` 2406–2456), which the
   server runs in `handlePlayerPositionChange` (`ServerPlayer.move(PLAYER, …)`,
   ServerGamePacketListenerImpl.txt 3279–3286), and `hit_ground` from
   `doCheckFallDamage` (3414, 3510). Clustine's players move by the client's word at
   the edge and are not moved by `Entity.move` in the region; the region has to run
   the player's step accumulation (`moveDist`, `nextStep`) and fall distance on the
   move the client reports, with the block under it, to post them.
4. **Sneaking matters, and is not region state.** `Player.getMovementEmission`
   (Player.txt 3978–3995) is `NONE` while flying or while on the ground with the shift
   key down, and `VibrationSystem$User.isValidVibration` (User.txt 32–90) drops any
   event in `#ignore_vibrations_sneaking` (`hit_ground`, `projectile_shoot`, `step`,
   `swim`, `item_interact_start`, `item_interact_finish`) whose source entity
   `isSteppingCarefully()` (= `isShiftKeyDown()`, Entity.txt 7636–7640). The region
   says today that "whether a player sneaks is not region state, so none steps
   carefully" (`region/pressed.rs`). N6 needs the shift key in the region. A creative
   player flying makes no steps at all.
5. **Stepping on a sensor activates it even when sneaking.** `SculkSensorBlock.stepOn`
   (128–181) skips `isValidVibration`, the occlusion and the dispatch: it asks
   `canReceiveVibration` with the sensor's own position and `step`, then
   `forceScheduleVibration` with the entity's position, so sneaking does not help
   (only a warden is let off). This needs `Block.stepOn` for any entity on the ground
   (the region has it for players, `pressed.rs`, and for carts and boats).
6. **The vibration is heard a tick late at the earliest, and its travel is
   `floor(distance)` ticks.** The listener adds a candidate with the game time;
   `VibrationSelector.chosenCandidate(now)` returns it only once `now` is past its tick
   (**Inferred**, the selector is not dumped), so the sensor's ticker takes it on the
   next tick, sets the travel time to `Mth.floor(distance)` (User.txt 26–30), sends
   the vibration particle, and receives when the travel time reaches 0 after the
   decrement of that same tick or a later one: received at tick `T + max(1,
   floor(d))` for an event at tick `T` at distance `d`. While the sensor holds a
   vibration it hears nothing else (Listener.txt 40–44).
7. **Several events in one tick: the closest wins, then the higher frequency**
   (**Inferred**, `VibrationSelector.shouldReplaceVibration`, not dumped): a later
   candidate of the same tick replaces the held one if it is strictly closer, or as
   close and of a strictly higher frequency; a candidate of a later tick than the held
   one does not replace it. The distance compared is the float `eventPos.distanceTo(
   listenerPos)`.
8. **Power is by block distance; the comparator reads the frequency.** On receipt the
   power is `max(1, 15 − floor(15.0 / radius × d))` (VibrationSystem.txt 43–59) with
   `d = (float) sqrt(BlockPos.containing(eventPos).distSqr(sensorPos))`
   (Listener.txt 131–138), radius 8 (16 calibrated, **Inferred**). The comparator
   reads `lastVibrationFrequency` while the sensor is `active`, else 0
   (SculkSensorBlock.txt 611–634): the sensor needs a typed block entity.
9. **The sensor waits for its 3×3 chunks to tick before it receives**
   (`requiresAdjacentChunksToBeTicking` true, VibrationUser.txt 124–127; Ticker.txt
   175–183, 211–257): a vibration whose travel is over stays held, retried every
   tick, while any of the nine chunks around the sensor is not loaded or does not
   tick. At a line between regions this has to be read as "the sensor's chunk ticks"
   (ADR-0025, section 3: a chunk along a border ticks when its 3×3 is loaded or
   copied), or a sensor beside a line would hold its vibration for ever (11).
10. **A vibration's source may be across a line, and so may the listener.** The
    dispatcher looks for listeners in every section within the event's notification
    radius (16, **Inferred**) and the sensor tests its own radius (8 or 16) by block
    distance; the listener reads the source entity's flags (spectator, shift,
    `dampensVibrations`), the affected block's tags and, for occlusion, up to six
    straight lines of blocks between the two positions (8.5). Section 11 of this
    brief has what an event effect has to carry.
11. **Calibrated sensors depend on a class not dumped.**
    `CalibratedSculkSensorBlock` (dumped) says only: `facing` from the player's
    horizontal direction, active for 10 ticks, no signal out of the side named by
    `facing` (54–68). The frequency filter from the back input and the radius 16 are
    in `CalibratedSculkSensorBlockEntity$VibrationUser`, not dumped (**Inferred**).
    Optional for N6; the plain sensor does not need it.
12. **No packet of a sensor's own**: its block entity has no update packet
    (`BlockEntity.getUpdatePacket` returns null, BlockEntity.txt; `SculkSensorBlockEntity`
    does not override it). The client sees the block state (phase, power, waterlogged)
    and the
    vibration particle (`level_particles`, 48, particle `vibration`, 58) sent to
    players within 32 blocks; the tendrils' animation and its particles are the
    client's (`animateTick`, 505–584, **Client**).

### 8.1 Game events: who posts them, where, with which context

`Entity.gameEvent(e)` = `level.gameEvent(this, e, this.position)` (Entity.txt
3571–3591); the `(Holder, BlockPos, Context)` and `(Entity, Holder, BlockPos)` overloads
post at `Vec3.atCenterOf(pos)` (**Inferred**: `LevelAccessor`, not dumped). On the
server they all reach `ServerLevel.gameEvent(e, Vec3, Context)` →
`gameEventDispatcher.post(e, pos, ctx)` (ServerLevel.txt n5 2372–2380). The context is
a source entity (or null) and an affected block state (or null).

Posted by entities (`Entity.txt`, n6 dump, unless said):

| Event | Where | When | Context |
|---|---|---|---|
| `step` | `vibrationAndSoundEffectsFromBlock` 2406–2456, from `applyMovementEmissionAndPlaySound` 2032–2142, from `move` (1491–1839, at 669–698) | `move` on the authoritative side, emission `emitsAnything`, not a passenger; `moveDist` (`+= horizontalDistance×0.6` or, on a climbable block or powder snow, `length×0.6`, as floats) has passed `nextStep`, the block at `getOnPos()` is not air; then for the block the entity is in (if not that one) and the on-block: the block is not air, the entity is on the ground, or climbing, or (not crouching or moving vertically) and on rails…, and not swimming; posted only when `emitsEvents`. `nextStep = (float)((int) moveDist + 1)` (3278–3286) | `(entity, state stepped on)`, at `entity.position()` |
| `swim` | `applyMovementEmissionAndPlaySound` 168–205 | as above but no block stepped and `isInWater()` | entity |
| `flap` | `processFlappingMovement` 2507–2521 | the on-block is air and `isFlapping()` (false for every N6 entity, 3829) | entity |
| `hit_ground` | `checkFallDamage` 3990–4042, from `move` 1778 (authoritative only) | on the ground with `fallDistance > 0`, after `Block.fallOn`; then the fall distance is reset. `fallDistance -= (float) dy` while not in water and moving down | `(entity, state at mainSupportingBlockPos, else the landing state)` at `entity.position` |
| `bounce` | `restituteMovementAfterCollisions` 1857–1998, at 263 | a bounce off a block (`Block.bounceOn`) | entity |
| `splash` | `doWaterSplashEffect` 4356–4579, from `updateFluidInteraction` 4284–4354 | first touch of water, not the entity's first tick | entity (after the splash's sound and particle draws) |
| `entity_die` | `kill` 650–658; `LivingEntity` 3690, `ServerPlayer` 1777 (n5), `ArmorStand`, `BlockAttachedEntity` (n5) | killed | entity |
| `entity_mount` | `startRiding` 6912–6994 | a ride begun (not forced on load) | passenger, at the vehicle |
| `entity_dismount` | `removeVehicle` 7051–7080 | a ride ended | passenger, at the vehicle |
| `entity_action` | `interact` 6475–6679 at 169–180 | leashes moved from the player to this entity | `(player)` at `blockPosition()` |
| `entity_interact` | `interact` 367–371 | a leash taken off | player |
| `shear` | `dropAllLeashConnections` 6714–6763 | leashes cut with shears | player |

Posted by the N5 code (n5 dump): `entity_place` by `MinecartItem`, `BoatItem`,
`ArmorStandItem`, `HangingEntityItem`, `CushionItem`; `entity_damage` by
`VehicleEntity`, `ArmorStand`, `LivingEntity` (5003), `Player` (1750); `equip`,
`unequip` by `LivingEntity.onEquipItem` (1446–1448); `block_attach` by
`LeashFenceKnotEntity` (221) and `LeadItem`; `block_change` by `ItemFrame` (386, 445,
889, four posts); `container_open`/`container_close` by `MinecartChest`,
`AbstractChestBoat`; `splash` by `AbstractBoat`; `entity_interact`, `shear`, `eat` by
`Mob`; `item_interact_start`/`_finish` and `elytra_glide` by `LivingEntity`;
`block_destroy` by `Level.destroyBlock` (n5 Level.txt 459–508: after the set succeeds,
`(entity breaking, old state)` at the position).

Posted by N6's classes (n6 dump):

| Event | Class, lines | When | Context |
|---|---|---|---|
| `prime_fuse` | `TntBlock.prime` 140–157 (post at 147–153) | TNT primed, after the `tnt.primed` sound | igniter entity, at the TNT's position |
| `explode` | `ServerExplosion.explode` 849– (first thing, 0–15) | every explosion, before its blocks are computed | the explosion's source entity, at `center` |
| `block_activate` | `DispenserBlock.dispenseFrom` 91– (65–86) | **only a failed dispense** (no item: level event 1001 then the event) | `(the dispenser's state)` |
| `block_activate`, `block_deactivate` | `BasePressurePlateBlock.checkPressed` 127– (176–205) | the plate's signal goes from 0 to more (activate) or to 0 (deactivate), with the click sound | the entity given to `checkPressed` (null from the plate's own tick) |
| `block_activate`, `block_deactivate`, `block_attach`, `block_detach` | `TripWireHookBlock.emitState` 481–559 | the hook's power on/off, attach/detach | null entity, at the hook |
| `shear` | `TripWireBlock.playerWillDestroy` 209– (236) | tripwire broken with shears | player |
| `container_open`, `container_close` | `ContainerOpenersCounter.incrementOpeners` 23– (42), `decrementOpeners` 65– (85), `recheckOpeners` 151– (211, 224) | first opener / last closer; recheck finds the count went to or from 0 | the user (open/close), null (recheck) |
| `container_open`, `container_close` | `ShulkerBoxBlockEntity.startOpen` 346– (386), `stopOpen` 409– (442) | first opener / last closer | the user |
| `fluid_place`, `fluid_pickup` | `DispenseItemBehavior$13` 104; `$4` 53, `$9` 14 | buckets dispensed | null (**Inferred**: dispensers post with no entity) |
| `entity_place` | `DispenseItemBehavior$6` 80, `SpawnEggItemBehavior` 65 | armour stand, spawn egg dispensed | |
| `block_place` | `DispenseItemBehavior$7` 48, `$8` 46, `FlintAndSteelDispenseItemBehavior` 51 | a block placed by a dispenser; fire lit | |
| `block_change` | `FlintAndSteelDispenseItemBehavior` 76 | a block lit (candle, campfire, TNT?) | |
| `shear` | `ShearsDispenseItemBehavior` 101, 155 | beehive or entity sheared | |
| `sculk_sensor_tendrils_clicking` | `SculkSensorBlock.activate` 380–452 (63–69) | a sensor activates | the vibration's entity, at the sensor; frequency 0 and not in `#vibrations`: only shriekers hear it |
| `resonate_1`…`resonate_15` | `SculkSensorBlock.tryResonateVibration` 453–504 | an amethyst block beside an activating sensor | `(entity, amethyst state)` at the amethyst block |

Not posted: a successful dispense of an item (the default behaviour posts nothing),
blocks broken by an explosion (only the one `explode`), a sensor's own state changes
(`setBlockAndUpdate` posts nothing). Arrows, snowballs, eggs and other projectiles
post `projectile_shoot`/`projectile_land` (N9). Items (`ItemEntity`) and falling
blocks have `MovementEmission.NONE` (n4-brief 1.0 item 14) and post no steps, but
`checkFallDamage` is not gated by the emission, so an item landing after a fall posts
`hit_ground` (**Inferred**: `ItemEntity` not dumped; it would need to override
`checkFallDamage` not to) and one entering water `splash`. Clustine's items keep no
fall distance today (`region/items.rs`); `hit_ground` for items needs it. `PrimedTnt`
has `MovementEmission.NONE` (PrimedTnt.txt), so TNT makes no steps; it lands
(`hit_ground`) as an item does. Minecarts and boats have `EVENTS` (n5
`AbstractMinecart`, `AbstractBoat`): they post steps and swims but play no step sound.

### 8.2 The dispatcher (**Inferred**: `GameEventDispatcher`,
`EuclideanGameEventListenerRegistry`, `GameEventListener` not dumped)

`post(event, pos, ctx)`:

1. `r = event.notificationRadius()` (16 for every event, **Inferred**);
   `b = BlockPos.containing(pos)`.
2. For section x from `(b.x − r) >> 4` to `(b.x + r) >> 4`, then z likewise, the chunk
   if loaded now (`getChunkNow`), then section y from `(b.y − r) >> 4` to
   `(b.y + r) >> 4`: each listener registered in that section, in the order
   registered. A listener is registered by its chunk when its block entity is added
   to a `ServerLevel` chunk, in its own position's section.
3. For each, the listener's position (`BlockPositionSource`: the block's centre,
   **Inferred**) `p`; if `BlockPos.containing(p).distSqr(b) > listenerRadius²` it is
   skipped. (The distances are between block positions, corner to corner, so the
   test is `dx² + dy² + dz² ≤ 64` in whole blocks for a sensor.)
4. A listener with `DeliveryMode.BY_DISTANCE` is queued and handled after, nearest
   first; every other is handled at once with `handleGameEvent(level, event, ctx,
   pos)`. `VibrationSystem$Listener` does not override `getDeliveryMode`
   (Listener.txt), so sensors are `UNSPECIFIED` and handled at once, in the visit
   order. For sensors the order does not matter: each has its own selector.

### 8.3 `VibrationSystem$Listener.handleGameEvent(level, event, ctx, pos)` (Listener.txt 30–92)

In order, returning false at the first that fails:

1. `data.currentVibration == null` (the sensor holds no vibration).
2. `user.isValidVibration(event, ctx)` (8.4).
3. The listener's position is present (`getPositionSource().getPosition(level)`).
4. `user.canReceiveVibration(level, BlockPos.containing(pos), event, ctx)`; for a
   sensor (VibrationUser.txt 48–74): false if the event's block is the sensor's own
   position and the event is `block_destroy` or `block_place`; false if the event's
   frequency is 0; else `SculkSensorBlock.canActivate(state)` = phase `inactive`
   (SculkSensorBlock.txt 336–346).
5. Not `isOccluded(level, pos, listenerPos)` (8.5).
6. `scheduleVibration` (111–129): `selector.addCandidate(new VibrationInfo(event,
   (float) pos.distanceTo(listenerPos), pos, ctx.sourceEntity()), level.getGameTime())`.

`forceScheduleVibration(level, event, ctx, pos)` (94–109, 230–242) does step 6 alone
(if the listener has a position): used by `stepOn` (8.12).

### 8.4 `isValidVibration(event, ctx)` (User.txt 32–90, the default; the sensor keeps it)

1. The event must be in `getListenableEvents()` = `#minecraft:vibrations` (User.txt
   11–14; the tag's 57 entries: every `block_*`, `bounce`, `container_*`, `drink`,
   `eat`, `elytra_glide`, `entity_*` but none other, `equip`, `explode`,
   `fluid_pickup`, `fluid_place`, `hit_ground`, `instrument_play`,
   `item_interact_finish`, `lightning_strike`, `note_block_play`, `prime_fuse`,
   `projectile_land`, `projectile_shoot`, `shear`, `splash`, `step`, `swim`,
   `teleport`, `unequip`, `resonate_1`…`15`, `flap`). Not in it: `item_interact_start`,
   `jukebox_play`, `jukebox_stop_play`, `sculk_sensor_tendrils_clicking`, `shriek`.
2. If there is a source entity: a spectator → false; if it `isSteppingCarefully()`
   and the event is in `#ignore_vibrations_sneaking` → false (and, for a sensor,
   `canTriggerAvoidVibration` is true, so a `ServerPlayer` gets the `avoid_vibration`
   advancement trigger, VibrationUser.txt 43–46); if `dampensVibrations()` → false
   (false for every entity but an item of wool, **Inferred**: `ItemEntity` overrides
   it; `Entity.dampensVibrations` 3954 is false).
3. If there is an affected state: false if it is in `#dampens_vibrations` (`#wool`,
   `#wool_carpets`, `#wool_slabs`, `#wool_stairs`): stepping on wool or carpet, or
   breaking or placing one, is not heard.
4. Else true.

### 8.5 Occlusion: `isOccluded(level, from, to)` (Listener.txt 140–228)

`a = (floor(from.x) + 0.5, floor(from.y) + 0.5, floor(from.z) + 0.5)` and `b` likewise
of `to`: the two block centres. For each `Direction` in `values()` order (down, up,
north, south, west, east), `a' = a.relative(dir, 1.0E-5)` (9.999999747378752E-6 as a
double constant) and `level.isBlockInLine(new ClipBlockStateContext(a', b, state ->
state.is(#occludes_vibration_signals)))`; if that hit is not `BLOCK`, the vibration is
**not** occluded (return false at once). Occluded only if all six lines hit. The tag is
`#wool` (16 wools; not carpets). `isBlockInLine` walks the blocks along the segment as
`BlockGetter.traverseBlocks` does (n5 dump `BlockGetter`; **Inferred**: the predicate
is tested on the block state alone, no shape) and stops at the first that matches. The
six nudges are what lets a vibration pass along a wool wall's edge.

### 8.6 The ticker: `VibrationSystem$Ticker.tick(level, data, user)` (Ticker.txt 3–51)

Run each tick by the sensor's block entity ticker (`SculkSensorBlock.getTicker`
282–294, server only, lambda 665–673; the calibrated one likewise, 29–40, 117–125), in
the block-entity phase:

1. If no current vibration: `trySelectAndScheduleVibration` (53–65, lambda 259–299):
   `selector.chosenCandidate(gameTime)`; if one: set it current; `travelTime =
   user.calculateTravelTimeInTicks(info.distance)` = `Mth.floor(distance)` (float,
   User.txt 26–30); send the particle `VibrationParticleOption(listener's position
   source, travelTime)` at the event's position, count 1, spread 0, speed 0
   (`ServerLevel.sendParticles`, to every player of the level whose block position's
   centre is within 32 blocks, strictly); `user.onDataChanged()` (the sensor's
   `setChanged`); `selector.startOver()` (the held candidate dropped).
2. If still none, return.
3. `changed = travelTime > 0`.
4. `tryReloadVibrationParticle` (67–157): only if the data's `reloadVibrationParticle`
   (true after a load from NBT, Data.txt 128–141): the particle again from the point
   `lerp(1 − travelTime / calculateTravelTime(distance), eventPos, listenerPos)` with
   the remaining travel time; the flag is cleared once it reached at least one player
   (a sensor loaded with nobody near sends it when someone comes).
5. `decrementTravelTime()` (`max(0, t − 1)`).
6. If `travelTime ≤ 0`: `changed = receiveVibration(…)` (159–209): the listener's block
   (`BlockPos.containing(listenerPos)`, else the event's block); if the user requires
   its adjacent chunks to tick (a sensor does) and any of the 3×3 chunks around that
   block is not `shouldTickBlocksAt` or not loaded (211–257), return false and keep the
   vibration (tried again next tick, travel time 0). Else `user.onReceiveVibration(
   level, eventBlock, event, entity (by UUID, else null), projectileOwner (else
   null), distanceBetweenInBlocks(eventBlock, listenerBlock))`, then current = null,
   true.
7. If `changed`, `user.onDataChanged()`.

`VibrationSelector` (**Inferred**, not dumped): it holds at most one `(info, tick)`.
`addCandidate(info, tick)` replaces it if none is held; else only if the held tick
equals `tick` and `info.distance < held.distance`, or equal distance and
`frequency(info) > frequency(held)`. `chosenCandidate(now)` gives the held info if its
tick `< now`. `startOver()` empties it. Its codec: `event` (optional `VibrationInfo`),
`tick` (long).

### 8.7 Frequencies: `VIBRATION_FREQUENCY_FOR_EVENT` (VibrationSystem.txt 61–326)

A `Reference2IntOpenHashMap` with default 0:

| Frequency | Events |
|---|---|
| 1 | `step`, `swim`, `flap` |
| 2 | `projectile_land`, `hit_ground`, `splash`, `bounce` |
| 3 | `item_interact_finish`, `projectile_shoot`, `instrument_play` |
| 4 | `entity_action`, `elytra_glide`, `unequip` |
| 5 | `entity_dismount`, `equip` |
| 6 | `entity_interact`, `shear`, `entity_mount` |
| 7 | `entity_damage` |
| 8 | `drink`, `eat` |
| 9 | `container_close`, `block_close`, `block_deactivate`, `block_detach` |
| 10 | `container_open`, `block_open`, `block_activate`, `block_attach`, `prime_fuse`, `note_block_play` |
| 11 | `block_change` |
| 12 | `block_destroy`, `fluid_pickup` |
| 13 | `block_place`, `fluid_place` |
| 14 | `entity_place`, `lightning_strike`, `teleport` |
| 15 | `entity_die`, `explode` |
| n | `resonate_n` (1 to 15, `RESONANCE_EVENTS`, static 327–415) |

Everything else (`item_interact_start`, `jukebox_*`, `sculk_sensor_tendrils_clicking`,
`shriek`) is 0, which a sensor refuses (8.3 step 4). `getGameEventFrequency(holder)`
(13–31) is 0 for an unbound holder.

### 8.8 `SculkSensorBlock`

State: `sculk_sensor_phase` (`inactive`, `active`, `cooldown`), `power` (0–15),
`waterlogged`; default `inactive`, 0, false (constructor 17–43). Shape
`Block.column(16, 0, 8)`: a half-block slab (static 760–779). `ACTIVE_TICKS = 30`,
`COOLDOWN_TICKS = 10`; `getActiveTicks()` 30 (375–379), 10 for the calibrated sensor.

- **Placing** (44–64): `waterlogged` if the clicked position's fluid is water.
  `getFluidState` (65–81) is still water while waterlogged.
- **`activate(entity, level, pos, state, power, frequency)`** (380–452), in order:
  `setBlockAndUpdate(pos, state[phase=active, power=power])` (flags 3);
  `scheduleTick(pos, block, getActiveTicks())`; `updateNeighbours` (257–272:
  `updateNeighborsAt(pos, block)` then `updateNeighborsAt(pos.below(), block)`);
  `tryResonateVibration(entity, level, pos, frequency)` (8.11); post
  `sculk_sensor_tendrils_clicking` with the entity at the sensor; if not waterlogged
  (the old state's flag), `playSound(null, x+0.5, y+0.5, z+0.5, sculk_clicking,
  BLOCKS, 1.0, level.random.nextFloat() × 0.2f + 0.8f)`.
- **`tick`** (82–127): if `active`, `deactivate` (347–374):
  `setBlockAndUpdate(pos, state[phase=cooldown, power=0])`, `scheduleTick(pos, block,
  10)`, `updateNeighbours`. If `cooldown`: `setBlockAndUpdate(pos,
  state[phase=inactive])`, and if not waterlogged `playSound(null, pos,
  sculk_clicking_stop, BLOCKS, 1.0, level.random.nextFloat() × 0.2f + 0.8f)`. If
  `inactive`, nothing.
- **`onPlace`** (182–217): unless the old state was the same block: if `power > 0` and
  no tick is scheduled for this block here, `setBlock(pos, state[power=0], 18)`
  (`UPDATE_CLIENTS | UPDATE_KNOWN_SHAPE`). (A sensor pasted or loaded powered
  without its tick goes dark; the phase stays.)
- **`affectNeighborsAfterRemoval`** (218–229): if it was `active`, `updateNeighbours`.
- **`updateShape`** (230–256): if waterlogged, schedule water's tick.
- **Signals**: `isSignalSource` true (300–304); `ownSignal = power` (305–313), on every
  side (the repo's `clustine_blocks::signal::own_signal` convention); `getDirectSignal`
  is `getSignal` for direction `UP` only, else 0 (314–327): the sensor strongly powers
  the block below it, which is why `updateNeighbours` updates around `pos.below()` too.
- **Comparator** (606–634): `hasAnalogOutputSignal` true; the output is the block
  entity's `lastVibrationFrequency` while `active`, else 0 (26.3's
  `getAnalogOutputSignal` takes a `Direction`; the sensor ignores it).
- **Breaking** (645–664): 5 experience if experience drops (never in creative).
- `isPathfindable` false; `useShapeForLightOcclusion` true.

### 8.9 `SculkSensorBlockEntity` and its `VibrationUser`

Block entity type `minecraft:sculk_sensor` (`BlockEntityTypes.SCULK_SENSOR`; the
calibrated one `minecraft:calibrated_sculk_sensor`). Fields: `lastVibrationFrequency`
(default 0), `vibrationData` (`VibrationSystem$Data`: `currentVibration`, `
travelTimeInTicks`, the selector, `reloadVibrationParticle`), the listener and the user
(constructor 13–38).

Saved (`saveAdditional` 81–97, `loadAdditional` 59–79):

```
{ last_vibration_frequency: int,
  listener: { event?: VibrationInfo, selector: VibrationSelector, event_delay: int ≥ 0 } }
```

`listener` is `VibrationSystem$Data.CODEC` (Data.txt 102–141): `event` lenient
optional (the current vibration), `selector`, `event_delay` (the travel time, default 0,
always written). A load builds the data with `reloadVibrationParticle = true`; a
missing `listener` gives a fresh `Data` (no vibration, empty selector, not reloading).
`VibrationInfo`'s codec (**Inferred**): `game_event` (registry id), `distance` (float),
`pos` (three doubles), `source` (UUID, optional), `projectile_owner` (UUID, optional).

The user (`SculkSensorBlockEntity$VibrationUser`): `LISTENER_RANGE = 8`
(`getListenerRadius` 32–35); position source `BlockPositionSource(blockPos)` (11–30);
`canTriggerAvoidVibration` true; `requiresAdjacentChunksToBeTicking` true;
`canReceiveVibration` as 8.3 step 4; `onReceiveVibration(level, eventBlock, event,
entity, owner, distance)` (76–115): if `canActivate(state)` (phase `inactive`):
`frequency = getGameEventFrequency(event)`; `setLastVibrationFrequency(frequency)`
(no `setChanged` of its own; the `setBlockAndUpdate` that follows updates the
comparator); `power = getRedstoneStrengthForDistance(distance, 8)`; if the block is a
`SculkSensorBlock`, `activate(entity, level, pos, state, power, frequency)`.
`onDataChanged` = `setChanged()` (117–122).

`getRedstoneStrengthForDistance(float d, int r)` (VibrationSystem.txt 43–59):
`max(1, 15 − Mth.floor((15.0 / (double) r) × (double) d))`. For r = 8: power 15 at
d < 0.5333…, 1 from d ≥ 7.4666…; the distance is between block positions, so at most
`sqrt(64) = 8` for a sensor (power 1).

### 8.10 `CalibratedSculkSensorBlock` (132 lines; **optional for N6**)

- `facing` (horizontal; default north), from `ctx.getHorizontalDirection()` at placing
  (42–52); `rotate`/`mirror` turn it (86–110).
- `getSignal(state, level, pos, dir)` is `ownSignal` unless `dir == facing`, then 0
  (54–68); `getDirectSignal` is inherited (strong power downwards only).
- `getActiveTicks()` 10 (112–115); the ticker is the same `Ticker.tick` (117–125) on a
  `CalibratedSculkSensorBlockEntity`.
- **Not dumped** (`CalibratedSculkSensorBlockEntity$VibrationUser`, **Inferred** from
  1.20): radius 16; `canReceiveVibration` additionally refuses an event whose frequency
  differs from the back input `getBackSignal` when that is above 0, the back input
  being `level.getSignal(pos.relative(facing), facing)` (the side the player faced);
  its `getRedstoneStrengthForDistance` uses 16.

What N6 needs for it: the block entity type with that user, the back-signal read, and
`facing` in `getSignal`.

### 8.11 Resonance: `tryResonateVibration(entity, level, pos, frequency)` (453–504)

For each `Direction` in `values()` order: the neighbour's state; if it is in
`#vibration_resonators` (`amethyst_block` only): post
`getResonanceEventByFrequency(frequency)` (= `resonate_<frequency>`, 33–41) at the
neighbour, context `(entity, neighbour state)`; play `amethyst_block_resonate`, BLOCKS,
volume 1, pitch `RESONANCE_PITCH_BEND[frequency]` at the neighbour (`playSound(null,
pos, …)`). `RESONANCE_PITCH_BEND[i] = NoteBlock.getPitchFromNote(NOTE[i])` with `NOTE =
{0, 0, 2, 4, 6, 7, 9, 10, 12, 14, 15, 18, 19, 21, 22, 24}` (lambda 675–759; the repo has
`clustine_blocks::note::pitch`, `2^((note − 12)/12)`). Other sensors within 8 blocks
of the amethyst then hear `resonate_n` with frequency n: a relay that keeps the
frequency.

### 8.12 Stepping on a sensor: `stepOn(level, pos, state, entity)` (128–181)

On the server, if `canActivate(state)` and the entity is not a warden and the block
entity at `pos` is a `SculkSensorBlockEntity`: if `user.canReceiveVibration(level, pos,
step, Context.of(state))` (true when inactive: `step` is not a place or destroy, its
frequency is 1), `listener.forceScheduleVibration(level, step, Context.of(entity),
entity.position())`. Then `super.stepOn`. No `isValidVibration` (sneaking, spectators,
wool), no occlusion and no check of a held vibration: if the sensor holds one still in
travel, the candidate waits in the selector, is chosen once the held one is received,
and is then refused at its own receipt by `canActivate` (the sensor is active by then)
(**Inferred**: the selector). `stepOn` runs from `Entity.applyEffectsFromBlocks`
(2294–2376) while the entity is on the ground, for the block at `getOnPosLegacy()` (0.2
below the feet, 2523–2529), every tick: an entity standing on an inactive sensor sets
it off again every 30 + 10 ticks and a tick for the selection. The entity stands at the
sensor's y + 0.5, so `BlockPos.containing(entity.position())` is the sensor's own block:
distance 0, power 15, frequency 1 (comparator 1); the travel distance (entity position
to the block's centre) is under 1, so it is received on the tick after the step.

### 8.13 What N6 builds for this

1. A game-event post in the simulation: `(event, Vec3, source entity key or none,
   affected state or none)`, posted from every place in 8.1 that Clustine runs, in the
   game's order within the tick (they are handled at once, not queued).
2. Listeners per chunk section: the sensors' typed block entities (ADR-0026 section 5),
   registered when their chunk is loaded or the block placed, in position order as
   the tickers are.
3. The dispatch (8.2), the listener (8.3–8.5), the selector and ticker (8.6), the
   frequencies (8.7), the block (8.8) and its block entity, durable as the game saves
   it (8.9).
4. The player's steps and falls on the moves the edge reports (8.0 item 3), the shift
   key in the region (item 4), items' fall distance (8.1).
5. The particle packet (`level_particles` with `vibration`) at the edge, sent within
   32 blocks; the sounds as block sounds.
6. Optional: calibrated sensors (8.10); shriekers wait for wardens (N9 or later).

### 8.14 Redstone ore stepped on by any entity (`RedStoneOreBlock`, for section 6)

`stepOn` (35–51): unless the entity `isSteppingCarefully()` (only a player with the
shift key down), `interact` (83–105): `spawnParticles` (169–272), then if not `lit`,
`setBlockAndUpdate(pos, state[lit=true])`. `spawnParticles` draws from
`level.getRandom()` on the server too (the particles go nowhere, `addParticle` is a
no-op on a `ServerLevel`, but the draws are made): for each `Direction` in order whose
neighbour is not `isSolidRender`, one `nextFloat()` for each of the two axes other than
the face's (two draws per open face, in x, y, z order). Clustine has this for players
(`region/pressed.rs`); N6 widens it to every entity on the ground (items, TNT, carts),
which never step carefully.

---------------------------------------------------------------------------------------

## 9. The packets and synched data of all the above, field by field

Gathered by subject from the parts; the ids are of `generated/reports/` (packets,
registries) for protocol 777.

### 9.1 TNT and the explosion (sections 1 and 2)

**Clientbound `explode` (protocol id 36)**, `ClientboundExplodePacket`
(ClientboundExplodePacket.txt 3–49; codec 138–162, `StreamCodec.composite` of eight):

| # | Field | Type | Codec |
|---|---|---|---|
| 1 | `center` | `Vec3` | `Vec3.STREAM_CODEC`: three `double`s, x, y, z (**Inferred**: `Vec3$1` is not dumped; the reference servers write three doubles) |
| 2 | `radius` | `float` | `ByteBufCodecs.FLOAT` (4 bytes) |
| 3 | `blockCount` | `int` | `ByteBufCodecs.INT` (**4 bytes, not a VarInt**) |
| 4 | `playerKnockback` | `Optional<Vec3>` | `Vec3.STREAM_CODEC.apply(ByteBufCodecs::optional)` (**Inferred**: a boolean, then the three doubles when present; the operation is a method reference not shown by `javap -c`) |
| 5 | `explosionParticle` | `ParticleOptions` | `ParticleTypes.STREAM_CODEC`: VarInt particle type id, then the type's options (none for `explosion` 30 and `explosion_emitter` 29) |
| 6 | `explosionSound` | `Holder<SoundEvent>` | `SoundEvent.STREAM_CODEC` (a holder: VarInt `id + 1` for a registered sound, 0 then an inline event; `entity.generic.explode` 703 → 704) (**Inferred** holder layout, as `sound` packets elsewhere in the edge) |
| 7 | `blockParticles` | `WeightedList<ExplosionParticleInfo>` | `WeightedList.streamCodec(ExplosionParticleInfo.STREAM_CODEC)` (**Inferred**: a VarInt count, then per entry the info — particle (as 5), a float scaling, a float speed — and a VarInt weight; neither class is dumped). The default list: `poof` (69) `0.5f, 1.0f`; `smoke` (72) `1.0f, 1.0f`; weights 1 |
| 8 | `playSound` | `boolean` | `ByteBufCodecs.BOOL`: `source == null || !source.isSilent()` |

Sent per player (ServerLevel.txt 2640–2686) to every player of the level whose
`distanceToSqr(center) < 4096`; `playerKnockback` is that player's entry in
`hitPlayers` (a survival or adventure player, or a creative one not flying, within the
damage range) or empty.

**Primed TNT** (`minecraft:tnt`, entity type 136): spawned by `add_entity` (1) with data
0 (**Inferred**: `PrimedTnt` does not override `getAddEntityPacket`) and the initial
motion; its synched data (`set_entity_data`, 101):

| Index | Field | Serializer id | Wire | Default |
|---|---|---|---|---|
| 8 | fuse | 1 (`INT`) | VarInt | 80 |
| 9 | block state | 14 (`BLOCK_STATE`) | VarInt of the state id | `tnt[unstable=false]` |

The fuse is dirty every tick, so `sendChanges` sends move, data and (when `needsSync`)
motion every tick; a collision sets `requiresPrecisePosition`, making the move an
`entity_position_sync`. Removal by `remove_entities` when it explodes.

**TNT minecart** (137): no synched data of its own (N5's 8–14 of `AbstractMinecart`);
priming is `entity_event` (34) with event 70.

**Sounds**: `entity.tnt.primed` (1627), block source, volume 1, pitch 1, at priming
(`sound`, 119, from `Level.playSound`). The explosion's own sound is in the packet.

**Game events** (not sent): `prime_fuse` (35) at priming (by the owner, at the block);
`explode` (23) at the centre.

### 9.2 Container blocks (3.1 to 3.9)

**Block events** (`Level.blockEvent` → `ClientboundBlockEventPacket`, built: position,
action id byte, action parameter byte, block id varint; **Inferred** field order from
the R-steps' encoder):

| Block | Action id | Parameter | Sent |
|---|---|---|---|
| chest, trapped chest, copper chests (each half of a double chest for itself) | 1 (`EVENT_SET_OPEN_COUNT`) | the half's new openers' count | on every change of the count, and on every recheck (every 5 ticks while open) |
| shulker boxes | 1 | the new `openCount` | on every `startOpen` and `stopOpen` |

The barrel sends none: its `open` property changes (a block update, flags 3).

**Sounds** (`ClientboundSoundPacket`, built for other sounds; source `blocks`, at the
position given, the seed from `soundSeedGenerator.nextLong()`):

| Sound (id) | Where | Volume | Pitch |
|---|---|---|---|
| `block.chest.open` 349 / `block.chest.close` 347 (copper chests: `block.copper_chest.open` 406 / `.close` 405; weathered 408 / 407; oxidized 410 / 409) | single: block centre; double: the `right` half's centre + its connected direction × 0.5 | 0.5 | `nextFloat() * 0.1f + 0.9f` |
| `block.barrel.open` 141 / `block.barrel.close` 140 | block centre + facing's unit vector / 2 | 0.5 | `nextFloat() * 0.1f + 0.9f` |
| `block.shulker_box.open` 1469 / `.close` 1468 | block centre | 0.5 | `nextFloat() * 0.1f + 0.9f` |
| `block.chest.locked` 348 | the container's centre (a double chest: the pair's centre) | 1.0 | 1.0 |

**System chat, overlay** (`ClientboundSystemChatPacket`, overlay true): `container.isLocked`
with the container's display name as its argument; `container.spectatorCantOpen` in red.

**Open screen** (`ClientboundOpenScreenPacket`, field by field in 3.10 on): container id
(1 to 100), menu type (`generic_9x3` 2, `generic_9x6` 5, `generic_3x3` 6, `crafter_3x3`
7, `hopper` 16, `shulker_box` 20), title (the custom name, or `container.chest`,
`container.chestDouble`, `container.barrel`, `container.shulkerBox`, `container.dispenser`,
`container.dropper`, `container.hopper`, `container.crafter`; a vehicle's display name).

**Block entity data** saved (the store's and the chunk's form; never sent):
`lock` (when locked, **Inferred**), `CustomName` (a text component, when named),
`LootTable` (identifier string) and `LootTableSeed` (long, when not 0) while a table is
set, else `Items` (list of `{Slot: byte, id, count, components}` for non-empty slots;
always written by chests and barrels, omitted when empty by shulker boxes),
`components` (always). Block entity type ids: `chest` 1, `trapped_chest` 2,
`ender_chest` 3, `dispenser` 5, `dropper` 6, `hopper` 18, `shulker_box` 24, `barrel` 26,
`crafter` 42.

**Game events** (section 8): `container_open` (11) and `container_close` (10), at the
block's centre, with the opener (the counter's first open, last close; the shulker
box's), or with no entity (a recheck's correction); for vehicles at the entity's
position.

### 9.3 Menus and clicks (3.10 to 3.21)

All ids from `generated/reports/packets.json` (play). `CONTAINER_ID` is
`ByteBufCodecs.CONTAINER_ID` (`ByteBufCodecs$24`, not read; a VarInt, **Inferred**, as the
edge's `SetContainerContent` writes it today and as SteelMC reads it).
`ItemStack.OPTIONAL_STREAM_CODEC` is the stack with its components (count VarInt, 0 for
empty; item VarInt; added components count, removed count, then each added as type VarInt
and
value, each removed as type VarInt: what `Writer::put_item_stack` writes with no
components).

**Serverbound**

| Packet (id) | Field | Codec | Notes |
|---|---|---|---|
| `container_click` (18) | `containerId` | `CONTAINER_ID` | ServerboundContainerClickPacket.txt 134–162 |
| | `stateId` | VarInt | |
| | `slotNum` | Short (signed 16 bits) | −999 outside, −1 margin |
| | `buttonNum` | Byte (signed 8 bits) | 3.14 per kind |
| | `containerInput` | `ContainerInput.STREAM_CODEC` | **Inferred** VarInt id 0–6 |
| | `changedSlots` | `ByteBufCodecs.map(Int2ObjectOpenHashMap::new, SHORT→int, HashedStack.STREAM_CODEC, 128)`: VarInt count (refused above 128, `readCount`, n5 ByteBufCodecs.txt 185–202), then per entry Short slot and `HashedStack` | the client's view after its prediction |
| | `carriedItem` | `HashedStack` | |
| `HashedStack` | present | Boolean (`ByteBufCodecs.optional`) | false = `EMPTY` |
| | item | `ByteBufCodecs.holderRegistry(ITEM)`: VarInt registry id (**Inferred**: not read) | HashedStack$ActualItem.txt 88–100 |
| | count | VarInt | |
| | added components | VarInt count ≤ 256, then per entry: component type (`ByteBufCodecs.registry(DATA_COMPONENT_TYPE)`, VarInt id) and Int (4 bytes, big-endian) | HashedPatchMap.txt 150–168 |
| | removed components | VarInt count ≤ 256, then component type VarInt each | |
| `container_close` (19) | `containerId` | `CONTAINER_ID` | ServerboundContainerClosePacket.txt 16–32 |
| `container_button_click` (17) | `containerId`, `buttonId` | not read (**Inferred** VarInt, VarInt) | ignored in N6 |
| `container_slot_state_changed` (20) | `slotId`, `containerId`, `newState` | not read (**Inferred** VarInt, VarInt, Boolean) | the crafter |
| `set_creative_mode_slot` (57) | `slotNum`, `itemStack` | not read (**Inferred** Short, then `ItemStack.OPTIONAL_UNTRUSTED_STREAM_CODEC`: components length-prefixed, as `Reader::untrusted_item_stack` reads) | already decoded by the edge |

**Clientbound**

| Packet (id) | Field | Codec | Notes |
|---|---|---|---|
| `open_screen` (60) | `containerId` | `CONTAINER_ID` | ClientboundOpenScreenPacket.txt 64–76 |
| | `type` | `ByteBufCodecs.registry(MENU)`: VarInt menu id | `generic_9x3` 2, `generic_9x6` 5, `generic_3x3` 6, `crafter_3x3` 7, `hopper` 16, `shulker_box` 20 |
| | `title` | `ComponentSerialization.TRUSTED_STREAM_CODEC` (a text component as NBT) | the provider's display name: `container.chest` and the like, or the custom name (3.1–3.9) |
| `container_set_content` (18) | `containerId` | `CONTAINER_ID` | ClientboundContainerSetContentPacket.txt 94–107 |
| | `stateId` | VarInt | |
| | `items` | `ItemStack.OPTIONAL_LIST_STREAM_CODEC`: VarInt count, then each stack | every slot of the menu, in slot order |
| | `carriedItem` | `ItemStack.OPTIONAL_STREAM_CODEC` | |
| `container_set_slot` (20) | `containerId` | `CONTAINER_ID` | ClientboundContainerSetSlotPacket.txt 56–77 |
| | `stateId` | VarInt | |
| | `slot` | Short | the menu's slot index; the server sends no negative one (the client's meanings of −1 and −2, if any, are **Client**) |
| | `itemStack` | `ItemStack.OPTIONAL_STREAM_CODEC` | copied at construction |
| `container_set_data` (19) | `containerId` | `CONTAINER_ID` | ClientboundContainerSetDataPacket.txt 44–60 |
| | `id` | Short | the data slot |
| | `value` | Short | written as 16 bits: values above 32767 are cut (the crafter's are 0 or 1) |
| `container_close` (17) | `containerId` | `CONTAINER_ID` | ClientboundContainerClosePacket.txt 26–32 |
| `set_cursor_item` (98) | `contents` | not read (**Inferred** `ItemStack.OPTIONAL_STREAM_CODEC`) | the carried stack, sent by the synchroniser's `sendCarriedChange` (**Inferred**) |
| `set_player_inventory` (110) | slot, stack | as the edge has it | not used by the menus; the edge's way for region-made changes |

When each goes:

- `open_screen` then `container_set_content` (state id incremented) and one
  `container_set_data` per data slot: at `openMenu` (3.10).
- `container_set_slot` (state id incremented each): from `broadcastChanges` for each slot
  whose
  real stack no longer matches the remote slot: after every click, at every player tick, and
  the other callers of 3.15.
- `set_cursor_item`: when the carried stack does not match the remote carried one.
- `container_set_content` again: after a click with a stale state id, and to a spectator
  or a
  dying player who clicked.
- `container_close`: when the server closes the menu (another opened, `stillValid`
  failed); not
  in answer to the client's close.

### 9.4 Dispensers, droppers and the crafter (sections 4 and 7)

**Level events** (`ClientboundLevelEventPacket`: type, position, data, global false; the
names from SteelMC's `level_events.json`, `LevelEvent` not dumped):

| Event | Number | Data | Where | Sent by |
|---|---|---|---|---|
| `SOUND_DISPENSER_DISPENSE` | 1000 | 0 | the dispenser | `DefaultDispenseItemBehavior.playDefaultSound` (150–159), `Optional` on success, `Boat`/`Minecart` behaviours' `playSound` |
| `SOUND_DISPENSER_FAIL` | 1001 | 0 | the dispenser or dropper | empty (`dispenseFrom`), `Optional` on failure |
| `SOUND_DISPENSER_PROJECTILE_LAUNCH` | 1002 (or the item's override) | 0 | the dispenser | `ProjectileDispenseBehavior.playSound` (86–102) |
| `PARTICLES_SHOOT_SMOKE` | 2000 | `facing.get3DDataValue()` (down 0, up 1, north 2, south 3, west 4, east 5) | the dispenser | `playDefaultAnimation` (161–171), after every `dispense` |
| `PARTICLES_AND_SOUND_PLANT_GROWTH` | 1505 | 15 | the front block | bone meal (`$5`) |
| `PARTICLES_WAX_ON` | 3003 | 0 | the front block | honeycomb (`$12`) |
| `SOUND_CRAFTER_CRAFT` | 1049 | 0 | the crafter | `dispenseItem`, per dropped stack |
| `SOUND_CRAFTER_FAIL` | 1050 | 0 | the crafter | no recipe or empty result |
| `PARTICLES_SHOOT_WHITE_SMOKE` | 2010 | `front.get3DDataValue()` | the crafter | after 1049 |

**Sounds** (`ClientboundSound`, source `blocks`, volume 1, pitch 1, the seed the level's:
the packet's seed is `level.random.nextLong()` in `ServerLevel.playSeededSound`,
**Inferred**, not read: one more level draw per sound): `entity.tnt.primed` at the
PrimedTnt's position (dispensed TNT, `TntBlock.prime`); `item.honeycomb.wax_on` at the
front block; `item.bottle.empty` at the dispenser (water potion to mud); `block.beehive.
shear` at the beehive (shears). The bucket's, the shulker box's place sound and fire's
come from the code those behaviours call (not read).

**Particles** (`ClientboundLevelParticles`): `splash`, five packets of count 1, offset 0,
speed 1, at `(pos.x + nextDouble(), pos.y + 1, pos.z + nextDouble())` of the dispenser
(water potion to mud).

**Block states**: `dispenser`/`dropper` `facing` (6) × `triggered` (2); `crafter`
`orientation` (12: `down_east`, … `west_up`, **Inferred** names `FrontAndTop`'s) ×
`triggered` × `crafting`. No block events.

**Block-entity data** (chunk NBT; none of these is sent to the client in the chunk
packet, **Inferred**: the dispenser's and crafter's `getUpdateTag` not overridden in what
was read): `dispenser`/`dropper`: `CustomName`, `lock`, `Items` (`Slot` byte + the item)
or `LootTable` + `LootTableSeed`. `crafter`: the same plus `crafting_ticks_remaining`
(int), `disabled_slots` (int array), `triggered` (int).

**Menus**: `generic_3x3` for dispensers and droppers (9 slots + 36), `crafter_3x3` for the
crafter (9 + 36 + the result slot 45, and ten data slots: 0–8 the slot states, 9
`triggered`, each sent as `container_set_data(id, index, value)`). Serverbound
`container_slot_state_changed`: container id (varint, **Inferred**), slot id (varint,
**Inferred**), new state (boolean); its codec not dumped.

**Entities made** (their spawn packets as for any): item entities (motion as 4.3, data
the stack), `tnt` (fuse 80, the block state data: section 1), boats and chest boats,
minecarts of six kinds, armour stands; equipment changes on players (their inventory
slot) and stands (`set_equipment`).

### 9.5 Hoppers, plates and tripwire (sections 5 and 6)

Hoppers, plates and tripwire add no packet of their own:

- **Block changes**: `ENABLED` of a hopper (flag 2), `POWERED` / `POWER` of plates (flag
  2), `POWERED` of tripwire (`setBlockAndUpdate`, flag 3), `DISARMED` (flag 260): each
  is a `block_update` (or a section update) as today.
- **Sounds**: plates' `pressurePlateClickOn`/`Off` of their block set (`sound`, source
  `BLOCKS`, volume 1, pitch 1, the seed); hooks' four sounds (6.5).
- **Hopper block entity**: no `block_entity_data` (`getUpdatePacket` is
  `BlockEntity`'s, null, BlockEntity.txt 525–528); in a chunk's data its entry carries
  `getUpdateTag` (**Inferred** empty for a hopper: not overridden, `BlockEntity.
  getUpdateTag` not read). Its contents travel only in the menu's packets (section 3).
- **Item entity partly taken**: `set_entity_data` with `ItemEntity.DATA_ITEM` (the
  stack left); wholly taken: `remove_entities`.
- **Hopper cart**: no synched data of its own (`Enabled` is not synched); its contents
  are not synched (only through its menu, section 3).

### 9.6 Sculk sensors and vibrations (section 8)

**Vibration particle.** `level_particles` (clientbound 48), sent by
`ServerLevel.sendParticles(option, x, y, z, 1, 0, 0, 0, 0)` (n5 ServerLevel.txt
2842–2859, through 2938/2958 with `RandomizationType.DEFAULT`, and the per-player send
3045–3061: same level, the player's block position's centre within 32.0, strictly):
`overrideLimiter` false, `alwaysShow` false, x, y, z (double) = the event's position
(or the interpolated point on a reload), the three spreads 0, speed 0, count 1, then the
particle: type `minecraft:vibration` (58) and `VibrationParticleOption`: the
destination `PositionSource` (type varint: `minecraft:block` 0 then the sensor's
`BlockPos` as a packed long; `minecraft:entity` 1 then the entity id varint and a float
y offset, for allays and wardens) and the arrival in ticks (varint, the travel time).
**Inferred**: the field order, the codecs of the option and the position source, and
where 26.3's new `RandomizationType` goes in the packet (`ClientboundLevelParticlesPacket`
not dumped; the existing comparisons with the official server should settle it).

**Block state.** `sculk_sensor`: `sculk_sensor_phase` (`inactive`, `active`,
`cooldown`), `power` 0–15, `waterlogged`; `calibrated_sculk_sensor` adds `facing`
(north, south, west, east). Sent as block updates; no block event, no block-entity
update packet (`getUpdatePacket` null).

**Sounds** (`sound` packets from `Level.playSound`, block source, the seed from the
level's `soundSeedGenerator`): `block.sculk_sensor.clicking` at the block's centre,
volume 1, pitch `0.8 + 0.2 × r` on activation; `block.sculk_sensor.clicking_stop` at the
block (`playSound(null, pos, …)`: the centre) on cooldown's end, same volume and pitch
form; neither when waterlogged. `block.amethyst_block.resonate`, volume 1, pitch of the
note table, at an amethyst block beside an activating sensor.

**Block entity data** (saved, `SculkSensorBlockEntity.saveAdditional` 81–97):
`last_vibration_frequency` (int), `listener` (compound: `event` optional
`VibrationInfo` {`game_event` id, `distance` float, `pos` [d, d, d], `source` UUID?,
`projectile_owner` UUID?}, `selector` {`event`? `VibrationInfo`, `tick` long},
`event_delay` int ≥ 0, always written). **Inferred**: the inner codecs of
`VibrationInfo` and `VibrationSelector` (not dumped).

**Game event ids** (`registries.json`, `minecraft:game_event`, for the codec of
`VibrationInfo` and any debug packet): block_activate 0, block_attach 1, block_change
2, block_close 3, block_deactivate 4, block_destroy 5, block_detach 6, block_open 7,
block_place 8, bounce 9, container_close 10, container_open 11, drink 12, eat 13,
elytra_glide 14, entity_damage 15, entity_die 16, entity_dismount 17, entity_interact
18, entity_mount 19, entity_place 20, entity_action 21, equip 22, explode 23, flap 24,
fluid_pickup 25, fluid_place 26, hit_ground 27, instrument_play 28,
item_interact_finish 29, item_interact_start 30, jukebox_play 31, jukebox_stop_play
32, lightning_strike 33, note_block_play 34, prime_fuse 35, projectile_land 36,
projectile_shoot 37, sculk_sensor_tendrils_clicking 38, shear 39, shriek 40, splash
41, step 42, swim 43, teleport 44, unequip 45, resonate_1 … resonate_15 46 … 60.

## 10. Random draws, in order, of each

Every draw is of the level's random (`level.random`, one stream per level) unless another
is named; the official server seeds it from the clock, so the oracle judges these by
bounds and distributions (n-plan section 5), and Clustine draws from its region's own
deterministic stream in the same order.

### 10.1 TNT and the explosion (sections 1 and 2)

**Priming a TNT block** (`TntBlock.prime`, 1.1; `wasExploded`, 1.2):
1. The entity's own random in `Entity.<init>`: the UUID, two `nextLong` (N4 brief 8.4).
2. The level's random: `nextDouble()` → angle `a = · 6.2831854820251465`.
3. Only from an explosion (`wasExploded`): the level's random `nextInt(20)` → fuse
   `+ 10`.
(The priming sound's seed is the sound seed generator's `nextLong`, not the level's.)

**The TNT minecart**:
1. `destroy` (priming by destruction): the cart's own random `nextInt(20)`, then
   `nextInt(20)` (fuse = sum).
2. `explode`: the cart's own random `nextDouble()` (power).

**The explosion** (`ServerExplosion.explode`), in order:
1. Rays: 1352 × the level's `nextFloat()` (i, j, k order; one per ray).
2. Entities, in query order: none of the explosion's own; what each death makes
   (items' own draws; a TNT minecart's two `nextInt(20)`; sound seeds).
3. Shuffle: the level's `nextInt(n)`, `nextInt(n − 1)`, …, `nextInt(2)`.
4. Per block in the shuffled order: the loot table's random sequence (none for TNT's
   own conditions by default; `survives_explosion` and each `explosion_decay` item one
   `nextFloat` each when the interaction is `DESTROY_WITH_DECAY`); the updates' own
   draws (a falling block's, a popped torch's `popResource`: three `nextDouble` and
   the item's own); a container's spill (per slot three `nextDouble`; per part one
   `nextInt(21)` and six `nextDouble` for the `triangle`s, **Inferred**; the item's own
   draws); a TNT's priming (as above: `nextDouble`, `nextInt(20)`).
5. Per collected stack: `popResource`'s three level `nextDouble`, then the item's own
   two `nextLong`, `nextFloat` (bob), `nextFloat` (yaw), two `nextDouble` (motion).
6. Fire (not TNT): per position one `nextInt(3)`.

Clustine's streams (N4 brief 8.5): the rays and the shuffle from one stream of the
explosion (the world, the TNT's key, the tick, a purpose mark), the loot and laying of
each block's drops from the block's stream as `drops.rs` does, the new entities' own
from their keys.

### 10.2 Container blocks (3.1 to 3.9)

In order, per action (the level's random unless said):

- **Opening a single chest or barrel (first opener)**: `onOpen` → one `nextFloat()` (the
  pitch), then the sound's seed `soundSeedGenerator.nextLong()`; nothing else.
- **Opening a double chest**: the `right` half first (`CompoundContainer.startOpen`): one
  `nextFloat()` and one seed; the `left` half draws nothing (its `playSound` returns
  before the draw). Closing likewise.
- **A recheck that changes the count to 0 or from 0**: as an open or close.
- **A shulker box's first open, last close**: one `nextFloat()`, one seed.
- **A lock's refusal**: one seed (pitch 1, no draw).
- **Breaking a container** (`Containers.dropContents`, per non-empty slot in slot
  order): `nextDouble()` × 3 (x, y, z), then per stack split off: `nextInt(21)` (the
  split's size), the `ItemEntity` constructor's own motion draws (**Inferred**: two
  `nextDouble()`), `triangle` × 3 (x, y, z motion; **Inferred** one `nextDouble()`
  each, `RandomSource.triangle` not read). An unopened loot table is first rolled by
  `LootTable.fill` with the seed's random (not read).
- **A creative player breaking a non-empty shulker box**: the `ItemEntity` constructor's
  motion draws (**Inferred**: two `nextDouble()`), at the block's centre.
- **Unpacking a loot table** (opening, reading, pulling, breaking): `LootTable.fill`'s
  rolls and its shuffle over the empty slots, all from the random of the seed
  (**Inferred**; not read). With a seed of 0 they come from the level's random, which
  the oracle cannot repeat.
- The shulker box's animation and entity push draw nothing.

### 10.3 Menus and clicks (3.10 to 3.21)

- No rule of a menu draws a random number. The `quickcraftSlots` set is a `HashSet` whose
  iteration order follows the slots' identity hashes; the result does not depend on it
  (3.14.1), so a port may use the slots' order.
- Items thrown out of a menu (`PICKUP`/`QUICK_MOVE` at −999, `THROW`, a `SWAP` whose
  stack does
  not fit back, and a carried stack dropped at closing by a removed or disconnected
  player) are
  `Player.drop(stack, includeThrower, prediction)` → `LivingEntity.drop` →
  `createItemStackToDrop(stack, false, includeThrower)` (n5 LivingEntity.txt 1657–1695):
  the N2
  throw from the eyes, with its draws from the player's random, in the order the items are
  thrown (`THROW` with button 1 over a result slot throws several, one after another). The
  closing drop (`includeThrower` false) is the same throw without the thrower recorded
  (**Inferred**: `Player.createItemStackToDrop` is N2's).
- `placeItemBackInInventory` draws only if it has to drop what does not fit (not read).

### 10.4 Dispensers, droppers and the crafter (sections 4 and 7)

**A dispenser's tick**, in order (the level's random unless "own"):

1. `unpackLootTable(null)` where a loot table is set (the table's own seeded random; not
   the level's: **Inferred**, `RandomizableContainer` not read).
2. `getRandomSlot`: one `nextInt(n)` per non-empty slot in slot order, `n` = 1, 2, 3, …
   (DispenserBlockEntity.txt 37–70).
3. The behaviour:
   - **drop** (default, and every fallback): the new `ItemEntity`'s own draws (two
     `nextLong` for the UUID, `nextFloat` bob, `nextFloat` yaw, two `nextDouble` motion:
     n4 8.4), then the level's `nextDouble` (speed) and three `triangle` (**Inferred** two
     `nextDouble` each): **7 level draws**. A fallback that dispenses twice (4.0 item 4)
     still drops once.
   - **remainder that does not fit**: another drop (7 level draws and the item's own six).
   - **TNT** (`$6`) and **flint and steel on TNT** (`TntBlock.prime`): the PrimedTnt's
     own UUID draws, then **one level `nextDouble`** (its direction); then the sound's
     seed (**Inferred**, `playSound` → one level `nextLong`).
   - **armour stand**: one level `nextFloat` (the discarded yaw, EntityType.txt 221–316),
     the stand's own UUID draws.
   - **boat, minecart**: their own UUID draws only.
   - **water potion to mud**: ten level `nextDouble` (five particle positions, x then z),
     then the sound's seed.
   - **honeycomb, beehive shear, glass bottle on a hive**: the sound's seed; the hive's
     `dropHoneycomb` → `Block.popResource` (n4 8.3's draws) for N7's case.
   - **flint and steel making fire**: fire's `onPlace` schedules its tick with
     `30 + nextInt(10)` (**Inferred**, Clustine's fire code: `random_ticks/fire.rs`);
     `hurtAndBreak` draws nothing without Unbreaking (**Inferred**).
   - **equipment, glowstone, shulker box, skull/pumpkin, empty bucket, full bucket**: no
     draw in what was read (the bucket's fluid sounds and the shulker box's place sound
     have seeds, **Inferred**; `emptyContents` in an ultrawarm dimension draws for its
     smoke: not read).
   - **projectiles** (N9): `Projectile.shoot`'s inaccuracy (`triangle`, the projectile's
     own random, **Inferred**); **spawn eggs** (N7): the mob's `finalizeSpawn`.

**A dropper's tick**: 1–2 as above; then `getContainerAt` on the front block: if no block
container and k ≥ 1 container entities meet the box, **one level `nextInt(k)`**; then
either `addItem` (no draw) or the drop (7 level draws and the item's own six).

**A crafter's tick**: no draw to choose (no slot is chosen); per `dispenseItem` call (the
result, then each non-empty remainder in grid order): the possible `nextInt(k)` of
`getContainerAt`, then where something remains, the drop's 7 level draws and the item's
own six.

**Streams for Clustine** (n4 8.5's proposal applied): the level's draws of one dispense
come from one stream of the block (world seed, the dispenser's position, the tick, a
purpose tag), drawn in the game's order above; the new entities' own draws from their
keys' streams. Distributions are the game's, values are not.

### 10.5 Hoppers, plates and tripwire (sections 5 and 6)

In the order they are drawn:

- **Hopper tick** (`pushItemsTick` → `tryMoveItems`), off cooldown and enabled:
  1. when the hopper is not empty, `ejectItems` → `getContainerAt(facing block)`: if no
     block container is there and at least one container entity's box meets the box of
     the facing block, `level.random.nextInt(count)` (one draw; with one entity too,
     **Inferred** that `LegacyRandomSource.nextInt(1)` draws, not read);
  2. when not full, `suckInItems` → `getContainerAt(block above)`: the same, for the
     block above.
- **Item entity meeting a hopper** (`HopperBlockEntity.entityInside` → `tryMoveItems`):
  draw 1 above when ejecting; the take from the entity draws nothing.
- **Hopper cart** (`tryConsumeItems` → `HopperBlockEntity.suckInItems`): draw 2 above for
  the position `containing(x, y + 1.5, z)` (a container entity centred there); the
  fallback over item entities draws nothing.
- **Loot tables**: a hopper reading a source with a loot table unpacks it (draws from its
  `LootTableSeed`; section 3), before anything else of that source.
- **Plates, tripwire, detector rails**: none. **Hooks**: the detach sound's pitch,
  `nextFloat()` (TripWireHookBlock.txt 538–551), built.

In Clustine the level's random is a stream of the region's (as `random_ticks.rs`'
`Random`); the hopper's draw needs a stream that does not depend on how many hoppers
ticked before it in the region, e.g. one keyed by the tick and the hopper's position
(**Inferred**: a choice for the plan; the oracle can judge it only by distribution).

### 10.6 Sculk sensors and vibrations (section 8)

- **`SculkSensorBlock.activate`**: one `level.getRandom().nextFloat()` (the clicking
  sound's pitch), after the state set, the scheduled tick, the neighbour updates, the
  resonance and the tendrils' event; none when the old state is waterlogged.
- **`SculkSensorBlock.tick`**, cooldown to inactive: one `level.getRandom().nextFloat()`
  after the set; none when waterlogged. Active to cooldown: none.
- **The vibration system** (listener, selector, ticker, travel, receipt, power,
  resonance): no draws. The sounds' seeds come from `soundSeedGenerator`, not the
  level's random.
- **The posts' callers**: `walkingStepSound` before a `step` (no draw of its own,
  **Inferred**: `Entity.playStepSound` uses the sound type's volume and pitch); the
  splash (`doWaterSplashEffect`, Entity.txt 4356–4579) makes its sound and particle
  draws from the entity's random before posting `splash`.
- **`RedStoneOreBlock.interact`** (for section 6): `spawnParticles` draws two
  `level.getRandom().nextFloat()` per face whose neighbour is not solid-render, faces in
  `Direction.values()` order (down, up, north, south, west, east), before lighting,
  even on the server.

## 11. What touches the line between regions

What each part reads or writes beyond the block or entity it starts from, and what that
means at a line between regions (ADR-0025, ADR-0026 section 10, ADR-0028 section 7).
Section 0 items 17 to 23 summarise it.

### 11.1 TNT and the explosion (sections 1 and 2)

- The explosion reads blocks within 6.9 blocks (rays) and 9 blocks (exposure clips) of
  its centre, and entities within 9: all within one chunk of the TNT's, so within the
  3×3 of copies. Reads across the line come from copies (a tick old while uncoupled).
- It writes blocks across the line as conditional sets carrying the explosion (for the
  holder to evaluate the loot with `THIS_ENTITY`, radius and owner, spill containers,
  prime TNT with its draws, and run the updates), and hits entities across the line by
  entity effects (damage source with the owner, amount, push). Drops of far blocks are
  laid and merged on the far side.
- The packet goes to players within 64 blocks in any region (the edge's fan-out).
- A primed TNT moves (N4's physics): it can cross a line while burning (N3's crossing,
  with its fuse, owner, block state and power in its record). It explodes in whichever
  region holds it when its fuse ends.
- TNT primed by redstone or fire at a line: the block's own region primes it; a signal
  across the line is read from the copy (ADR-0026 section 10).
- The game event `explode` reaches sculk sensors across the line (section 8's
  matter).
- Where regions follow players, an explosion near a line is entities working at the line
  (ADR-0028 section 7): merge.

### 11.2 Container blocks (3.1 to 3.9)

- **The menu joins the player's inventory to the container's slots.** Every click
  moves stacks between them (a quick move, a swap, a drag across both), so a menu has to
  run where both are: the player's region must hold the container's chunk (with both
  halves of a double chest), or the container's region must have the player. Proposed
  (**Inferred**, for the plan): a container opens only where the player's region holds
  every chunk its container lies in; where regions follow players they merge as when
  entities work at their line (ADR-0028 section 7); at a pinned line the click on a
  container across the line opens nothing, a named difference, as ADR-0026's piston
  moves only within land it holds. A player's reach (5 in creative) lets a player open a
  container up to about five blocks into the other region.
- **A double chest across a chunk boundary can be across a line**: the combiner reads
  the partner's block entity, which a border copy has only as data (copies are never
  written). Opening it there is the item above; a comparator or a hopper reading it reads
  the copy (the 54 slots' signal needs the partner's items as of the copy).
- **The recheck counts the region's own players**: a player who has a chest open while
  standing in another region (when the menu stays valid there, 8.5 to 9 blocks) is not
  found by `getEntitiesWithContainerOpen`; the lid closes and `CONTAINER_CLOSE` is sent
  after 5 ticks. With the rule above (open only in the player's region) the opener is
  always the container region's own, but a player who walks across the line with the
  menu open leaves the region: the menu is then to be closed on crossing (as a cart
  leaving range closes it), **Inferred**, or the recheck taught about guests.
- **A comparator reading a container across a line** reads the container's slots from
  the copy: copy updates carry block changes (ADR-0025 section 3); whether they carry a
  block entity's data, and so whether the reading follows the contents, is to be
  checked; a change of contents tells the comparator through
  `updateNeighbourForOutputSignal`, which crosses as an update effect.
- **The shulker box's push** may reach beyond its chunk when the box faces across a
  chunk edge: entities in the band that another region has are moved by an entity effect
  (a push, ADR-0028 section 7) or not at all; the box's neighbour updates cross as
  updates already do.
- **The cat on a chest** (N7) is an entity query in the block above, always in the
  chest's own chunk column.
- **Sounds and block events** are broadcast by the region that holds the block, as
  built.
- **Breaking a container spills its items in its own region** (the drop positions are
  within the block's box).
- **Container vehicles** are their region's: a cart or chest boat whose menu is open and
  that crosses a line closes the menu (**Inferred**: the game's `stillValid` would keep
  it; the region that has the entity is the one that can run the menu).

### 11.3 Menus and clicks (3.10 to 3.21)

- **Opening across a line.** A chest, barrel, dispenser, dropper, hopper, crafter or shulker
  box opens by `useWithoutItem` on its block, which ADR-0026 section 7 refuses on land the
  player's region does not hold. To open it, the menu would need the container's holder
  to run
  the clicks (it changes the container's slots) and the player's region to change the
  inventory, in one step, which two regions cannot do without lending. Proposal: keep the
  refusal (the click does nothing) and name it; where regions follow players they merge when
  players work at a line (ADR-0028), so the seam remains only at pinned lines. A player sees
  it within minutes only if a pinned line runs through a storage room: the plan should
  say so.
- **A double chest across a line** (its halves in two regions): the `CompoundContainer`
  reads
  both halves; opening needs both in the player's region (3.1–3.9 say what the block does);
  with one half foreign, refuse as above (**Inferred** proposal: open neither, rather than a
  single chest's menu, which would show the half differently from the game).
- **A player whose region changes with a menu open** (riding a minecart or a boat, pushed,
  falling; a creative player cannot walk with a screen open, **Client**): in the game the
  menu
  stays while `stillValid` (within reach + 4 of the block) holds. Across a transfer the old
  region keeps the container and the new one the inventory. Proposal: the leaving region
  closes
  the menu as the game's `closeContainer` does (the carried stack put back into the
  inventory
  before the inventory goes with the transfer, `stopOpen` run so the lid closes and the
  openers'
  count falls), and the edge sends `container_close`. A named difference that shows only
  to a
  player riding with a chest open.
- **A container entity crossing with a menu open on it** (a chest or hopper minecart, a
  chest
  boat, N5's): in the game the menu lives while the cart is within reach + 4
  (`ContainerEntity.isChestVehicleStillValid`, N5 brief 3.18). Proposal: a cart's transfer
  closes every menu open on it in the region it leaves, as `stillValid` failing would;
  viewers
  in the cart's new region then reopen it.
- **The carried stack and duplication.** A click moves items between the container and the
  inventory; both must change in the same commit of one region, as they do when the region
  holds both. The carried stack is not durable in the game (lost if the server stops);
  here it
  is lost if the region restarts with a menu open, as in the game, but at a transfer, a
  split or
  a merge it must be put back into the inventory first (closing the menu), never copied.
- **Two viewers of one container in two regions** cannot happen with the refusal above; with
  it, the openers' count (and the lid) is the holder's alone.

### 11.4 Dispensers, droppers and the crafter (sections 4 and 7)

What a dispenser, dropper or crafter at `pos` facing `f` reads and writes outside its own
block (the front `F = pos + f`):

- **Reads**: `hasNeighborSignal` of `pos` (six neighbours) and, for the dispenser and
  dropper, of `pos.above()` (its six neighbours, so blocks up to two away); the block and
  fluid at `F`; for boats and carts the block or fluid at `F.below()`; for the shulker box
  `F.below()`; entities whose boxes meet the unit box of `F` (equipment, sulfur cubes,
  armadillos, horses, shearable or leashed entities); for a dropper or crafter the block
  entity at `F` (and, for a chest, the chest beside `F` that makes it double, so up to
  two blocks from `F`), else container entities in the box of half-size 0.5 around `F`'s
  centre (which can reach into `F`'s neighbours' chunks only when `F` lies at a chunk's
  edge).
- **Writes**: a block at `F` (fire, mud, a skull, a carved pumpkin, a shulker box,
  water or lava, a drained fluid, a waxed or lit or charged block, TNT removed) with its
  updates; entities made at `pos + 0.7 f` (items: inside `F`'s block space, or for an
  item's fall into `F`'s neighbour), at `F`'s centre (TNT, armour stand), 1.125 out
  (carts) or 1.25 out (boats) along `f`, so in `F` or the block beyond; equipment set on
  an entity in `F`; a leash cut on an entity in `F`; a container's slot at `F` changed
  (and a hopper's cooldown there).
- **At a line**: `F` and the entities in it may be another region's. The block writes are
  "conditional sets" of ADR-0025 section 4 (fire, mud and the like: an expected block and
  a state); but most behaviours *decide* on what they read at `F` (is it water, a rail, a
  respawn anchor at charge below 4, an empty slot) and change the dispenser's own slot by
  the outcome, so they cannot be split into a read here and a write there without the
  item being lost or doubled when the other side refuses. Options for the plan:
  (a) as ADR-0026's piston, **a dispenser, dropper or crafter acts only when `F` (and
  `F.below()`, and where a chest at `F` is double, its other half) is land the region
  holds and ticks**; otherwise it waits (its tick rescheduled, or kept as "a waiting
  dispenser" tried again when the land changes), a named difference that a player at a
  pinned line sees as a dispenser that does nothing until the regions merge;
  (b) where regions follow players, **merge at once** when a dispenser fires at the line
  (ADR-0026 section 10's rule for redstone, ADR-0028's for entities);
  (c) for the plain drop only (the item entity made at `pos + 0.7 f`, which may lie in the
  other region's chunk), make the entity in the dispenser's region and let it cross as
  any item does (N3), which needs no effect.
  **Inferred** recommendation: (c) for drops, (a) with (b) for everything else; equipment
  and leashes go as entity effects (ADR-0028 section 7) only if (a) is not taken.
- **A dropper inserting across a line** is a container write whose failure keeps the item
  in the dropper; no effect today carries it. Under (a) it waits; an "insert one, or send
  it back" effect is the alternative, but it changes the order of the dropper's slot and
  the receiver's against the game's single tick.

### 11.5 Hoppers, plates and tripwire (sections 5 and 6)

- **Pushing across a line.** A hopper at a chunk edge facing north, south, east or west
  ejects into a block that may be another region's. The move reads the target's block
  state, its block entity (a chest's double half may be in a third chunk), and the
  container entities whose boxes meet the target block's box (any chunk around it), and
  then writes, in one step: one item out of the hopper's slot, into the target's slot,
  and, when the target is an empty hopper, the target's cooldown, read against the
  target's `tickedGameTime`. Nothing of that can be split into conditional sets without
  losing or doubling items. Options for the plan: (a) as the piston's rule (ADR-0026
  section 10), the hopper moves only where the target block's chunk and every chunk the
  entity box meets are held and ticking by the region, and waits otherwise (the push
  stalls at a pinned line; a player watching a hopper chain across a line would notice
  within minutes); (b) an offer: the hopper takes the item out and sends it as an effect
  "insert this stack at pos from face"; the holder inserts what fits by its own rules
  (`addItem`, its own cooldown hand-over with `k = 0`) and returns what does not fit as an
  effect back, which the hopper puts back into the same slot or, if that is no longer
  possible, the first slot that takes it, else drops it at the hopper. (b) keeps the
  chain moving at a tick or two of delay per crossing and is exact in count; it changes
  timing (a full target is learnt a tick late, so the hopper's cooldown is set as if it
  moved) and the comparator readings at the line. **Inferred** (a proposal).
- **Pulling never crosses by blocks.** The container a block hopper pulls from is the
  block above, in the same chunk column, so the same region. What can cross: the
  **entities** found by box (item entities in the suck box whose position, and so chunk
  and region, is across the line while their box overlaps the hopper's column; container
  carts and chest boats centred across the line), and for a hopper cart the block at
  `containing(x, y + 1.5, z)` and its item search box (`inflate(0.25, 0, 0.25)`), which
  straddle a line when the cart does. An entity is changed only by its holder: an item
  across the line is not taken until it or the hopper's region holds it (as ADR-0028
  section 7 says for pick-ups), and a cart across the line is not pulled from or pushed
  into (it is skipped as if absent, which also changes the draw's count).
- **A hopper cart crossing** carries its contents, `enabled` and nothing else (no
  cooldown, `consumedItemThisFrame` is per tick).
- **Plates and tripwire at a line.** The touch box and tripwire's shape lie within their
  block, so within one chunk; but an entity whose box meets it can be held across the
  line (a cart 0.98 wide straddling, a boat, an item at the edge). Its `entityInside`
  runs in the region that holds the entity, which does not hold the plate: it becomes an
  effect to the plate's holder, "an entity (not ignoring triggers; living or not) came
  in at pos", on which the holder runs `checkPressed` (plates: a recount; tripwire:
  pressed by that entity). The recount and the scheduled re-checks count the holder's own
  entities and those of its copies across the line (a tick old while coupled; copies
  carry entities, as `minecarts_meeting` reads them), so a plate is kept pressed by an
  entity across the line and released a tick or so late. ADR-0026 section 10's "plates
  and tripwire see the region's own players" becomes "the region's own entities and its
  copies' entities". A weighted plate's count mixes current and tick-old entities at a
  line. **Inferred** (a proposal).
- **Detector rails** are N5's: the same as plates (it reads carts of copies already,
  `minecarts_meeting`).
- **An entity in flight between regions** is in neither for that time (n-plan, risks):
  missed by plates, tripwire, detector rails and hoppers for that tick.

### 11.6 Sculk sensors and vibrations (section 8)

- **An event and a listener on two sides of a line.** The dispatcher visits every
  section within the event's radius (16, **Inferred**) of the event's block, so a post
  in one region reaches sensors whose block is within 8 (by `distSqr` of block
  positions) in another. What the listener reads: its own state (held vibration,
  phase), the event (id, position as a `Vec3`, game time), the context (the source
  entity's `isSpectator`, `isSteppingCarefully`, `dampensVibrations`, its UUID; the
  affected state), and blocks for occlusion: six segments from the event block's
  centre (nudged 1e-5 each way) to the sensor block's centre, each walked until a wool
  block. Proposed: the posting region sends an **event effect** to the chunk of each
  foreign sensor in range it can see (its copy shows the sensor's block entity) or,
  simpler, to every foreign chunk the dispatch would visit within 8 blocks (sensor
  radius; 16 if calibrated sensors are built), carrying (event, position, tick,
  source entity key and those three flags, affected state); the holder runs
  `handleGameEvent` on its own land and copies. The occlusion's segments reach at most
  8 (16) blocks into the poster's side, inside the border copies (ADR-0025 section 3:
  the 3×3 of a viewer's chunk), and a block the holder can read neither in its land
  nor a copy should count as not occluding (**Inferred** choice). The candidate's tick
  is the poster's tick: while coupled the same, uncoupled a tick or so late, which
  delays the receipt by as much.
- **The vibration's entity on receipt.** `VibrationInfo.getEntity` looks the source up
  by UUID in the level; across a line it is absent and `null` is passed to `activate`
  (it only becomes the context of the tendrils' and resonance events). Harmless for
  sensors; shriekers and wardens (later) use it.
- **"Adjacent chunks ticking".** The receipt waits while any of the sensor's 3×3
  chunks is not loaded and ticking; a border copy is neither. Read it as the sensor's
  chunk ticking (which ADR-0025 grants when its 3×3 is loaded or copied), or a sensor
  in the first chunk beside a line never fires.
- **Stepping on a sensor across a line.** `stepOn` runs in the stepping entity's
  region for the block 0.2 below its feet, which can be a foreign chunk's sensor; it
  writes that sensor's selector. It has to go as an effect (`forceScheduleVibration`
  with event `step`, the entity's position and key) to the sensor's holder, as other
  `stepOn` effects of `pressed.rs` would.
- **The sensor's own effects** (state set, scheduled tick, neighbour updates at `pos`
  and `pos.below()`, resonance posts at its six neighbours) are ordinary block
  changes and updates and cross a line as ADR-0025 says.
- **The particle** goes to players within 32 blocks of the event: the edge shows it to
  any of its players within 32, whichever region they are in (as other particles and
  sounds go).

## 12. Not read

What a rule still depends on and nobody has read, with what it is needed for, by subject.
Read by more than one part, and so first to dump: `RandomSource.triangle` (spilled
contents, dispensed items), `GameRules` (the defaults of `tnt_explodes` and the
drop-decay rules), `RandomizableContainer` (container loot),
`LevelEntityGetter`/`EntitySectionStorage` (the order of entity queries: who is hurt,
sucked or counted first), `ItemEntity` (explosion damage, merging, `hit_ground`, hoppers
taking items), `Attributes` (`block_interaction_range` for menus' validity, explosion
knockback resistance), `CrafterSlot`/`NonInteractiveResultSlot`/`ResultContainer` (the
crafter's menu), `ByteBufCodecs.optional` (packet bytes) and `ItemStack.hurtAndBreak`
(flint and steel, shears).

### 12.1 TNT and the explosion (sections 1 and 2)

- Not read: `BlockBehaviour.onExplosionHit` (and `BlockStateBase.onExplosionHit`),
  needed for the loot parameters of a block broken by an explosion (`THIS_ENTITY`,
  `EXPLOSION_RADIUS` only with decay, `BLOCK_ENTITY`), `spawnAfterBreak`, the set's
  flags and the call of `wasExploded` (2.6 is **Inferred** from 1.21).
- Not read: `Block.getExplosionResistance` with `Blocks` (each block's `strength` /
  `explosionResistance`) and `FluidState.getExplosionResistance` (`WaterFluid`,
  `LavaFluid`, `EmptyFluid`), needed for every ray step; Clustine's data has no
  resistance; the datagen's `Extract.java` can write it.
- Not read: `FireBlock` (`checkBurnOut`), needed for the order of fire's replacing the
  TNT and priming it (Clustine's `fire.rs` ports the rest).
- Not read: `GameRules`, needed for the defaults of `tnt_explodes`,
  `tnt_explosion_drop_decay`, `block_explosion_drop_decay`, `mob_explosion_drop_decay`,
  `mob_griefing` (taken from SteelMC's 26.3 extraction: true, false, true, true, true).
- Not read: `DamageSources.explosion(Entity, Entity)` and `explosion(Explosion)`,
  needed for which damage type (`explosion` or `player_explosion`) a TNT with or without
  an owner gives, and which of the two entities is the cause.
- Not read: `ItemEntity.hurtServer`, `ItemEntity.areMergable` and `ItemEntity.merge`
  (in this dump's era), needed for an item's explosion damage (`damage_resistant`) and
  the drops' merging (taken from the N4 brief).
- Not read: `FallingBlockEntity.hurtServer` and `ignoreExplosion`, needed for what an
  explosion does to a falling block.
- Not read: `Vec3$1` (`Vec3.STREAM_CODEC`), `WeightedList` (its stream codec and the
  builder's default weight), `ExplosionParticleInfo` (its fields and codec),
  `ByteBufCodecs.optional`, needed for the exact bytes of the explode packet.
- Not read: `Level$ExplosionInteraction` and `ServerLevel$1` (the switch map), needed
  to confirm which interaction maps to which block interaction.
- Not read: `LevelEntityGetter` / `EntitySectionStorage`, needed for the order in which
  the explosion's entities are hurt (and whether the box test is strict).
- Not read: `ClipContext` and `BlockGetter.traverseBlocks` in 26.3 form (the latter is
  in the n5 dump, BlockGetter.txt 154–), needed for the exposure's clip with
  `COLLIDER` and `Fluid.NONE` and the entity's collision context.
- Not read: `ItemStack.hurtAndBreak` and `consume` for a creative player (in the n5
  dump's `ItemStack`, not reread), needed for a flint and steel's and fire charge's use
  on TNT in creative.
- Not read: `ShulkerBoxBlockEntity.preRemoveSideEffects` (dumped, section 3's), needed
  for what an exploded shulker box keeps.
- Not read: `Attributes` (the default of `explosion_knockback_resistance`, **Inferred**
  0), needed for the knockback of living entities (N7).
- Not read: `RandomSource.triangle`, needed for the draws of a spilled container.

### 12.2 Container blocks (3.1 to 3.9)

- Not read: `DoubleBlockCombiner` (and its `NeighborCombineResult`, `Combiner`), needed
  for the exact pairing of chest halves and when a blocked half makes the whole chest
  open nothing (3.3; **Inferred** from source).
- Not read: `TrappedChestBlock` and `TrappedChestBlockEntity`, needed for the trapped
  chest's signal, its neighbour updates on a change of openers, and its statistic.
- Not read: `CopperChestBlock` and `WeatheringCopperChestBlock`, needed for which copper
  chests join, their sounds by oxidation (the exposed ones) and their weathering.
- Not read: `EnderChestBlock`, `EnderChestBlockEntity` and `PlayerEnderChestContainer`,
  needed for the ender chest's menu (the player's own 27 slots, `container.enderchest`),
  its lid and its blocked test.
- Not read: `RandomizableContainer` (the interface: `unpackLootTable`,
  `tryLoadLootTable`, `trySaveLootTable`), needed for exactly when a loot table is rolled
  and how `LootTable`/`LootTableSeed` are written.
- Not read: `LootTable.fill` (and `getAvailableSlots`, `shuffleAndSplitItems`), needed
  for how container loot is spread over the slots and which random it draws from.
- Not read: `LockCode` (and `ItemPredicate`), needed for what unlocks a lock and the
  `lock` key's form.
- Not read: `BaseEntityBlock` (`getMenuProvider`, `triggerEvent`), needed for the
  spectator's path to non-chest containers and for forwarding block events to the block
  entity (**Inferred**).
- Not read: `BlockBehaviour.useItemOn` (its `TRY_WITH_EMPTY_HAND` default) and
  `getCollisionShape`, needed for a click with an item in hand reaching `useWithoutItem`
  and for the shulker box's collision shape.
- Not read: `Shulker.getProgressAabb` and `getProgressDeltaAabb`, needed for the shulker
  box's shapes, the room its lid needs and the band whose entities it pushes.
- Not read: `LevelChunk.setBlockState`, needed for when `preRemoveSideEffects` (the
  spilling of contents) runs and which flags skip it.
- Not read: `LevelAccessor.gameEvent(Entity, Holder, BlockPos)`, needed for the game
  event's position (**Inferred** the block's centre).
- Not read: `ServerLevel.doBlockEvent` / `runBlockEvents`, needed to confirm a block
  event is sent whenever `triggerEvent` answers true (the chest's every recheck).
- Not read: `Attributes` (and the creative mode's attribute modifiers), needed for
  `block_interaction_range` (4.5, and 5.0 in creative), which sets the menus' validity
  and the recheck's box.
- Not read: `ItemStackWithSlot` and the item stack's codec, needed for the `Items`
  entries' exact NBT.
- Not read: `RandomSource.triangle` and the five-argument `ItemEntity` constructor,
  needed for the draws of spilled contents.
- Not read: `Cat.isInSittingPose`, needed (N7) for the cat's blocking.
- Not read: `Entity.getDisplayName` for vehicles, needed for a container vehicle's menu
  title.

### 12.3 Menus and clicks (3.10 to 3.21)

- Not read: `net.minecraft.world.inventory.ContainerInput` (its order and `STREAM_CODEC`),
  needed for the wire ids of the click kinds (**Inferred** 0–6 in the order of 3.0 item 15).
- Not read: `ClickAction`, needed only to confirm `PRIMARY`/`SECONDARY` (trivially from
  `getClickAction`).
- Not read: `RemoteSlot` and `RemoteSlot$Synchronized`, needed for exactly how a remote slot
  compares a forced stack and a received hash (`matches`, `force`, `receive`, `copyFrom`).
- Not read: `ServerPlayer$1` (the `ContainerSynchronizer`) and `ServerPlayer$2` (the
  `ContainerListener`), needed for which packets `sendInitialData`, `sendSlotChange`,
  `sendCarriedChange` and `sendDataChange` send and when each increments the state id, and
  for the hash generator the remote slots use.
- Not read: the `HashGenerator`'s implementation (`HashOps` and the CRC32C over a
  component's
  encoding), needed to match a client's hash of a stack with components.
- Not read: `ByteBufCodecs$24` (`CONTAINER_ID`), `ByteBufCodecs.holderRegistry`,
  `ByteBufCodecs.optional` and `ByteBufCodecs$28` (`map`), needed for the exact bytes of the
  container id, the hashed stack's item and the changed-slot map.
- Not read: `ServerboundContainerButtonClickPacket`,
  `ServerboundContainerSlotStateChangedPacket`,
  `ServerboundSetCreativeModeSlotPacket` and `ClientboundSetCursorItemPacket`, needed for
  their
  fields and codecs.
- Not read: `InventoryMenu`, `ArmorSlot` (or the inventory menu's armour slot class),
  `ResultSlot`, `CraftingContainer`/`TransientCraftingContainer`, needed for the player's
  own
  menu (the 2×2 grid's items, `removed`, the armour slots' `mayPlace`/`setByPlayer`).
- Not read: `ShulkerBoxSlot`, needed for its `mayPlace` (which items a shulker box's menu
  refuses).
- Not read: `CrafterSlot`, `NonInteractiveResultSlot`, `ResultContainer`,
  `SimpleContainerData`,
  `DataSlot`, needed for the crafter menu's slots (a disabled slot's `mayPlace`, the result
  slot's `mayPickup`) and data slots' update flags.
- Not read: `Inventory.placeItemBackInInventory`, `Inventory.add`,
  `Inventory.getItem`/`setItem`,
  needed for where a carried stack goes at closing and for `SWAP`'s overflow.
- Not read: `ItemStack.overrideStackedOnOther`, `overrideOtherStackedOnMe` and `BundleItem`,
  needed for bundles in menus (not N6's).
- Not read: `MenuProvider`/`SimpleMenuProvider`, needed for the providers' display names
  (the titles of `open_screen`).
- Not read: `Attributes` (the `block_interaction_range` default) and the game mode's
  attribute
  modifiers, needed for `stillValid`'s distance (4.5 + 4, and creative's, **Inferred** 5
  + 4).
- Not read: `PacketUtils.ensureRunningOnSameThread` and the handling of a
  `ReportedException`
  from a packet, needed only to say what the game does with a malformed `SWAP` or drag (the
  port refuses it either way).

### 12.4 Dispensers, droppers and the crafter (sections 4 and 7)

- Not read: `RandomizableContainer` (the interface's `unpackLootTable`,
  `tryLoadLootTable`, `trySaveLootTable`), needed for when and with what random a
  dispenser's structure loot is unpacked (4.1).
- Not read: `Container.getMaxStackSize(ItemStack)`, needed for `insertItem`'s limit (4.1)
  and the crafter's one-at-a-time rule (7.2).
- Not read: `RandomSource.triangle`, needed for how many draws `spawnItem` makes (4.3).
- Not read: `GameRules` (`TNT_EXPLODES`'s default), needed for whether dispensers prime
  TNT by default (4.10).
- Not read: `BucketItem.emptyContents`/`checkExtraContent` and `BucketPickup.pickupBlock`
  of `LiquidBlock`, `SimpleWaterloggedBlock` and `PowderSnowBlock`, needed for what a
  dispenser's bucket places or picks up, and its sounds and game events (4.5).
- Not read: `BaseFireBlock.canBePlacedAt`/`getState`, `CampfireBlock.canLight`,
  `CandleBlock.canLight`, `CandleCakeBlock.canLight`, needed for flint and steel (4.11).
- Not read: `BoneMealItem.growCrop`/`growWaterPlant` and the `BonemealableBlock`s, needed
  for bone meal from a dispenser (4.12).
- Not read: `WitherSkullBlock.canSpawnMob`/`checkSpawn`, `CarvedPumpkinBlock.
  canSpawnGolem`, `RotationSegment.convertToSegment`, needed for the skull's rotation and
  the golem and wither patterns (4.13).
- Not read: `HoneycombItem.getWaxed`, `RespawnAnchorBlock.charge`, `BeehiveBlock.
  releaseBeesAndResetHoneyLevel`/`dropHoneycomb`, needed for honeycomb, glowstone and
  bottles on hives (4.14).
- Not read: `BlockItem.place` and `DirectionalPlaceContext`, needed for a shulker box
  placed by a dispenser (4.15).
- Not read: `Entity.shearOffAllLeashConnections`, needed for shears cutting leashes from a
  dispenser (4.16; it is in the dumped Entity.txt but was not read for this section).
- Not read: `ItemStack.hurtAndBreak`, needed for flint and steel's and shears' wear and
  whether it draws (4.11, 4.16).
- Not read: `Direction.toYRot` and `get3DDataValue`, needed for the armour stand's and
  boat's yaw from a vertical dispenser and the smoke's data (4.6, 4.7; **Inferred** the
  usual 0–5 and `(get2DDataValue & 3) · 90`).
- Not read: `EntitySelector.CONTAINER_ENTITY_SELECTOR` (a `javap -v` for the lambda),
  needed for which entities a dropper or crafter can put into (4.18).
- Not read: `ProjectileItem` and its `DispenseConfig` for each projectile, and
  `Projectile.spawnProjectileUsingShoot`, needed for N9's dispensed projectiles (4.17).
- Not read: `SulfurCube` (`equipItem`, `primeTime`) and `SpawnEggItem`, needed for N7's
  cubes and eggs from dispensers (4.10, 4.16).
- Not read: `RecipeCache`, `CraftingInput`, `RecipeManager`, the crafting recipe classes
  (`ShapedRecipe`, `ShapelessRecipe`, `TransmuteRecipe` and the special ones),
  `CraftingRecipe.getRemainingItems`, `ItemStack.onCraftedBySystem`, needed for the
  crafter's craft (7.2).
- Not read: `CrafterSlot`, `NonInteractiveResultSlot`, `ResultContainer`, and
  `ServerboundContainerSlotStateChangedPacket`'s codec, needed for the crafter's menu
  (7.4).
- Not read: `FrontAndTop` (its value names and `fromFrontAndTop`), needed for the
  crafter's `orientation` values (7.1).
- Not read: `LevelEvent`, needed for the level events' names (quoted from SteelMC's
  extraction).
- Not read: `ServerLevel.playSound`/`playSeededSound` and `addFreshEntity`, needed for
  whether a sound's seed is a level draw and whether adding an entity makes a game event.

### 12.5 Hoppers, plates and tripwire (sections 5 and 6)

- Not read: `BlockSetType` (and `BlockSetType$PressurePlateSensitivity`), needed for which
  plates are `MOBS` and which `EVERYTHING`, and for their click sounds.
- Not read: `Blocks`, needed for the weighted plates' `maxWeight` (15 and 150, as
  `pressed.rs` has them) and for which blocks are hoppers' containers by class.
- Not read: `WorldlyContainer`, `WorldlyContainerHolder`, `ComposterBlock` (and its
  `InputContainer`/`OutputContainer`/`EmptyContainer`), needed for the composter as a
  hopper's source and target.
- Not read: `AbstractFurnaceBlockEntity`, `BrewingStandBlockEntity`,
  `ChiseledBookShelfBlockEntity`, `DecoratedPotBlockEntity`, `JukeboxBlockEntity`,
  `ShelfBlockEntity`, needed for which other block entities are containers to a hopper
  and their faces' slots and `canPlaceItem` rules (a creative player meets them all).
- Not read: `LevelChunk$BoundTickingBlockEntity`, needed for which state the ticker
  passes (the chunk's current state, **Inferred**) and that a block entity whose state no
  longer fits is not ticked.
- Not read: `ItemEntity` (in neither dump), needed for `setItem`'s and `discard`'s effect on
  the entity's data and whether a hopper may take an item with a pick-up delay (it does
  not look at it: **Inferred** it may).
- Not read: `LegacyRandomSource.nextInt`, needed only to know that `nextInt(1)` draws
  (the oracle cannot compare the level's random anyway).
- Not read: `Block.column` and `Block.boxZ`, needed for the argument order of the
  hopper's and plates' shapes (**Inferred** from their values; the repo's constants agree
  for plates and tripwire).
- Not read: `LevelEntityGetter` and `EntitySectionStorage`, needed for the order of
  `getEntitiesOfClass` and `getEntities` (which item a hopper takes first, the list the
  draw indexes).
- Not read: `ServerPlayerGameMode.destroyBlock` for creative breaking's call of
  `playerWillDestroy` (shears disarming tripwire): in the N5 dump, not re-read here.
- Not read: `RandomizableContainer.unpackLootTable` (interface default) and
  `RandomizableContainer.tryLoadLootTable`, needed for a loot chest's unpacking by a
  hopper (section 3 may have it).

### 12.6 Sculk sensors and vibrations (section 8)

- Not read: `GameEvent` (the registry entries and `notificationRadius` of each event),
  needed for the dispatcher's search radius (**Inferred** 16 for all).
- Not read: `GameEventDispatcher.post`, needed for the sections searched, the order of
  listeners and the queue of `BY_DISTANCE` listeners.
- Not read: `GameEventListener` (`getDeliveryMode`'s default) and
  `GameEventListenerRegistry`/`EuclideanGameEventListenerRegistry`, needed for whether
  sensors are handled at once (**Inferred** `UNSPECIFIED`) and for the range test
  (**Inferred** `distSqr` of block positions ≤ radius²).
- Not read: `LevelChunk.addGameEventListener`/`removeGameEventListener` (and
  `getListenerRegistry`), needed for when a sensor starts and stops listening (placed,
  loaded, unloaded, removed) and in which section.
- Not read: `VibrationSelector`, needed for which of several events in a tick is chosen
  and when (`addCandidate`, `chosenCandidate`, `startOver`, its codec).
- Not read: `VibrationInfo`, needed for its codec (saved in the sensor's `listener`)
  and `getEntity`/`getProjectileOwner`.
- Not read: `BlockPositionSource`, `EntityPositionSource`, `PositionSource(Type)` and
  `VibrationParticleOption`, needed for the listener's position (**Inferred** the
  block's centre) and the particle's wire format.
- Not read: `ClientboundLevelParticlesPacket` (and its `RandomizationType`), needed for
  the particle packet's fields in 26.3.
- Not read: `LevelAccessor`/`LevelWriter` `gameEvent` overloads, needed for the
  position a block-position post uses (**Inferred** `Vec3.atCenterOf`) and the
  `ResourceKey` overload used by resonance.
- Not read: `Level.isBlockInLine` and `ClipBlockStateContext`, needed for the walk of
  the occlusion's segments (which blocks a segment visits, whether the start block
  counts).
- Not read: `CalibratedSculkSensorBlockEntity` and its `VibrationUser`, needed for the
  calibrated sensor's radius, its frequency filter and its back input.
- Not read: `ItemEntity`, needed for whether items post `hit_ground` on landing and
  whether a wool item `dampensVibrations`.
- Not read: `Block.playerWillDestroy`, `BlockItem.place`, `LeverBlock`, `ButtonBlock`,
  `DoorBlock`, `TrapDoorBlock`, `FenceGateBlock`, `NoteBlock`, `PistonBaseBlock`,
  `BellBlock`, `RepeaterBlock`, `ComparatorBlock` (only for its posts; it has none in
  the dump), `BucketItem`, `FlintAndSteelItem`, `FallingBlockEntity`: needed for the
  game events the blocks and items already built post (all **Inferred** in 8.1).
- Not read: `Entity.playStepSound`/`walkingStepSound`'s sound-type lookup, needed only
  if step sounds are sent as well.
- Not read: `SculkShriekerBlock`, `SculkShriekerBlockEntity`, `Warden`, `Allay`: the
  other vibration listeners, needed only when they are built (after N6).

