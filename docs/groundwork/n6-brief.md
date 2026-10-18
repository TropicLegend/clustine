# N6 porting brief: TNT, explosions, containers and the redstone that waits for entities (26.3, protocol 777)

Read from `server-26.3.jar` with `javap -p -c -constants` (names are Mojang's), one file
per class, and from `target/datagen/26.3/generated/`. Every rule below is the bytecode's
unless marked **Inferred** (reasoning, to test) or **Client** (client code, not in the
server jar). Method names are the game's; "line" ranges, where given, are of the `javap`
output of that class ("ServerExplosion.txt" is `net.minecraft.world.level.
ServerExplosion`'s, and so on), so a porter can go straight to them. The step's own dump
is `reference/n6-javap/`: 158 classes first, then 171 more that the first reading's
section 12 named (`more.txt` lists them), from which the brief was completed; the N5 dump, `reference/n5-javap/`, has the rest
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
a value in a class not dumped (level event names, fluids' resistance), and says so where
it is.

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
   as a VarInt; an unknown id reads as `PICKUP`), hashed stacks (the client's hashes
   are CRC32C over a tagged encoding of each component, `HashOps`, 3.16), a state id,
   `container_set_content`/`_slot`/`_data`, `set_cursor_item`, closing (3.0 items 15–26; 3.10–3.21). Chests, barrels and shulker
   boxes come with them at little more cost, with their openers' count, lids, block
   events and sounds (3.1–3.5).
2. **The split between the edge and the region falls on the game's own seam,
   `ContainerSynchronizer`** (ADR-0005): the region runs `AbstractContainerMenu`'s rules
   on the container and inventory it holds; the edge keeps the container id (1 to 100),
   the state id (15 bits; moved only by `container_set_content` and
   `container_set_slot`, while `set_cursor_item` and `container_set_data` carry none),
   the remote slots and their hashes, and sends every packet
   (3.0 items 17, 19; 3.21). A click whose state id is stale still runs, and the whole
   menu is sent again after it; a click on another container id, a menu no longer
   valid or a slot out of range is refused without an answer (3.0 item 16).
3. **Game events do not exist in Clustine**, and N4 and N5 skipped every post. Sculk
   sensors (section 8) need them posted from each place the game makes one: steps,
   landings, splashes, riding, N5's entities placed, item frames, levers, buttons,
   doors, pistons, note blocks, plates, tripwire, containers opening and closing, TNT
   primed, explosions, blocks broken and placed. Items and falling blocks post
   `hit_ground` when they land (neither overrides `checkFallDamage`), so items need a
   fall distance; a wool item's events are dampened. The protocol crate's `GameEvent` is the
   unrelated "change game state" packet; the simulation's need another name (8.0 items
   1, 2). Players' steps and falls are heard only if the region runs them on the moves
   the edge reports, and **sneaking has to become region state** (`region/pressed.rs`
   takes no player as sneaking today) (8.0 items 3, 4). Sculk sensors could be left out
   of N6 and named; if they are kept, the posts are most of their work.

**What the data lacks**

4. **Block explosion resistance is missing** from Clustine's data and from
   `reports/blocks.json`. The second dump gives every value (2.2's table, read from
   `Blocks`: stone 6, planks 3, glass 0.3, obsidian 1200, TNT 0); the datagen's
   `Extract.java` should still write `getExplosionResistance()` per block, since a block
   class may change its properties in its constructor (the infested blocks,
   **Inferred**), and the table is its check. Fluids' (100 for water and lava) are still
   SteelMC's, so a waterlogged block resists 100 (2.0 item 10; 2.2).
5. **Item stacks have no components** (`clustine_world::entity::ItemStack`). Menus work
   without them, but a shulker box keeps its contents in its item (`container`
   component, `copy_components` in its loot), so a shulker box broken by a creative
   player, by an explosion or dispensed would lose its contents; named and enchanted
   stacks lose their components through a menu, and a client's hash with components
   never matches, so the slot is resent at each click. **A decision for the plan**:
   components in stacks in N6, or shulker boxes named as a difference (3.0 item 18; 2.6;
   3.5). A shulker box item destroyed as an entity (by an explosion or fire) spills its
   contents in the game (`BlockItem.onDestroyed`); without components there is nothing
   to spill.
6. **Container loot tables are missing**: structure chests, barrels, dispensers and
   hoppers keep `LootTable`/`LootTableSeed`, unpacked when first opened, read by a
   comparator, pulled from by a hopper, dispensed from or broken; Clustine generates only
   block loot tables (3.0 item 11; 5.0 item 7; 4.0 item 2).

**What 26.3 is**

7. **There is no `onCaughtFire`** (a Forge hook): fire burns TNT in
   `FireBlock.checkBurnOut`, which first replaces the TNT with fire or air (the TNT's
   burn odds are 100 against `nextInt(300)`, 250 above and below) and then primes it
   with no owner; Clustine's `region/random_ticks/fire.rs` already marks the place. Every
   priming draws one `nextDouble` of the level's random for the motion's angle (turned by
   `Math.sin`/`Math.cos`); a TNT primed by an explosion draws `nextInt(20)` too, for a
   fuse of 10 to 29 ticks. `PrimedTnt` has no `baseTick` (no void check, no burning);
   its fuse is synched data that changes every tick, so it is sent every tick; it
   explodes with radius 4 at `y + 0.06125` (1.0 items 1–5).
8. **TNT drops everything it breaks**: its interaction is `DESTROY` while the game rule
   `tnt_explosion_drop_decay` is false (its default, GameRules.txt 790–800), so the
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
11. **New game rule `tnt_explodes`** (default true, GameRules.txt 777–787) gates
    `PrimedTnt.explode`, `TntBlock.prime` and the dispenser's TNT; when it is off a flint
    and steel on TNT answers with the overlay `block.minecraft.tnt.disabled`, and fire
    still burns the TNT away without priming it (1.1; 4.0 item 8). The other defaults
    the brief relies on are read too: `tnt_explosion_drop_decay` false,
    `block_explosion_drop_decay` and `mob_explosion_drop_decay` true, `mob_griefing`
    true, `block_drops` true (2.1).
12. **Dispensers and droppers fire 4 ticks after a rising edge** of
    `hasNeighborSignal(pos) || hasNeighborSignal(pos.above())` and do not look at the
    power again; the slot is one level `nextInt(n)` per non-empty slot (reservoir
    sampling, `n = 1, 2, …`). 82 items are registered by name; equippable items go onto
    players and armour stands in front; **fire charges are projectiles in 26.3**, and the
    13 projectile items wait for N9 (dropped as items until then is recommended, named).
    A dropper puts one item into the container in front silently and keeps it when it
    does not fit. Several fallbacks play the click and smoke twice. Shears cut N5's
    leashes. The sulfur cube (a stable 26.3 mob) is N7's, but its tag is checked on every
    dispense of a block. A dispensed shulker box is placed through `BlockItem.place` with
    a context of no player looking `DOWN`, and not into an entity's box. A dropped item
    takes seven draws of the level's random (`triangle` is two `nextDouble`); sounds take
    none, since their seeds come from a generator of their own (4.0; 4.3; 4.4–4.18;
    9.4).
13. **Hoppers move items in the entity phase as well as their own**: an item entity's
    block walk meeting a hopper runs the hopper's whole `tryMoveItems` (eject, then take
    that entity). Clustine's item walk (`physics/item.rs`) is pure and has to hand
    hoppers, plates, tripwire and detector rails back to the region, as `CartInside`
    does. The cooldown is 8, handed on to an empty hopper as 8 or 7 by whether it has
    ticked this game tick (`tickedGameTime`, not saved). A hopper's choice among
    container entities is a draw of the level's random. A hopper cart has no cooldown,
    never pushes, and keeps an `Enabled` flag (saved, not synched) that Clustine's
    minecart record lacks. The composter is a hopper's source and target through its
    `LEVEL` alone: a hopper above composts one item per move, a hopper below takes the
    bone meal (and loses it if it cannot hold it), and a composter above a hopper stops
    it sucking items (5.0; 5.6).
14. **Plates count every entity but spectators and marker armour stands** (stone plates
    living ones only, so in N6 only players and armour stands); a weighted plate counts
    entities, not items, `ceil(min(n, max)/max · 15)`; only the stone and polished
    blackstone plates are `MOBS`; every wooden plate (poplar included) and both weighted
    plates are `EVERYTHING` (`BlockSetType`, 6.2). Tripwire is
    pressed by the entity that walked in and kept by any entity. A detector rail needs
    only that any entity entering it tells comparators (6.0).
15. **Containers**: every container block consumes the click, opening or not; a chest
    under a conductor opens nothing and reads 0 to a comparator; a double chest is one
    `CompoundContainer` with the `right` half first, two counts, two block events, one
    sound at the pair's centre; chests and barrels recheck their openers every 5 ticks by
    a scheduled block tick and send the block event each time; the shulker box's lid is
    simulated on the server, changes its shape and pushes entities; breaking any
    container but a shulker box spills its contents, in creative too; no container sends
    `block_entity_data`. Copper chests join any copper chest, both halves becoming the
    less oxidised block, which N1's placement does not do yet; the trapped chest is a
    signal source of its openers' count (0 to 15, strong downwards); the ender chest
    opens the player's own 27 slots, which N2's player data has to keep; a lock also
    opens for any spectator (3.0 items 1–14).
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
       fluids, attached blocks popping off); a container **spills** its contents (per stack a `nextInt(21)` and six
       `nextDouble`, `triangle` being two draws, then the item's own); a TNT
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
    spills the container, primes TNT (a TNT burnt by fire across the line
    likewise: the fire's effect carries "burnt" and the holder primes it after the set)
    and lays the drops, which **cannot be merged with
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
    and a vibration within 8 blocks (16 for a calibrated sensor) of a sensor across the
    line has to go as an effect carrying the event, its tick, the source's flags
    (spectator, sneaking, wool) and the affected block, since the game's dispatch finds
    listeners only in loaded chunks and a copy has none; the occlusion reads up to 8 blocks into the far side, within the
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
   Forge hook, not the game's. Fire primes TNT from `FireBlock.checkBurnOut` (FireBlock
   .txt 651–712; 1.1), which calls `TntBlock.prime(level, pos)` itself **after** it has
   replaced the TNT with fire or air. TNT's burn odds are 100 (`setFlammable(TNT, 15,
   100)`, FireBlock.txt 1471–1474), so a fire beside it burns it when its `nextInt(300)`
   (`nextInt(250)` above or below) is under 100. Clustine's port says where:
   `region/random_ticks/fire.rs`, "Burnt TNT would be primed here". The ways TNT is
   primed are, then: redstone (placed powered,
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
count changes: `consume(n, entity)` shrinks the stack only when the entity is null or
has no infinite materials (ItemStack.txt 2486–2496), and `hurtAndBreak(n, entity,
slot)` (859–886) → `hurtAndBreak(n, level, player, onBreak)` (744–762) →
`processDurabilityChange` (764–790) answers 0 for a `ServerPlayer` with infinite
materials, before any enchantment (Unbreaking) draw. The set is flag 11,
not `removeBlock`'s 3: the neighbours are updated and the client sent the change, and the
set is immediate. The order is: entity, sound, game event, then the block goes. Whether
the block's `useItemOn` is reached at all with a flint and steel follows
`ServerPlayerGameMode.useItemOn` (N5 brief 2.9): a sneaking player holding an item skips
the block and uses the item, so a flint and steel then sets fire beside the TNT (which
primes it only when the fire burns it).

**Burnt by fire: `FireBlock.checkBurnOut(level, pos, chance, random, age)`** (FireBlock
.txt 651–712). The fire's tick (`tick`, 247–) calls it for its six neighbours in the
order east, west, below, above, north, south, with `chance` 300, 300, 250, 250, 300,
300, less 50 each where the environment attribute `increased_fire_burnout` holds at the
fire (FireBlock.txt 386–404), `random` the level's and `age` the fire's:

```
odds = getBurnOdds(level.getBlockState(pos))          // 0 if waterlogged; TNT 100
if random.nextInt(chance) < odds:                     // draw 1
    old = level.getBlockState(pos)
    if random.nextInt(age + 10) < 5 && !level.isRainingAt(pos):     // draw 2
        newAge = min(age + random.nextInt(5) / 4, 15)                // draw 3
        level.setBlockAndUpdate(pos, getStateWithAge(level, pos, newAge))   // fire, flag 3
    else:
        level.removeBlock(pos, false)                                // air, flag 3
    if old.getBlock() is TntBlock:
        TntBlock.prime(level, pos)                    // no owner; draws as 1.3
```

So the TNT's place is already fire or air (with its updates run) when the entity is
made; `prime` does not remove anything itself, and a `false` answer (the game rule
`tnt_explodes` off) leaves the TNT burnt and gone. `getStateWithAge` sets the fire's
`age` only if `getState(level, pos)` is fire (it may be soul fire, which has none).
Clustine's `region/random_ticks/fire.rs` ports the rest of the fire; the priming goes
where it marks.

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
applyGravity()                                              // motion.y -= 0.04 (Entity.txt 3924–3942; 0 with NoGravity)
move(SELF, getDeltaMovement())                              // N4's Entity.move
applyEffectsFromBlocks()                                    // N4's: cobweb, honey, fire, …
setDeltaMovement(getDeltaMovement().scale((double) getAirDrag()))   // 0.98f widened: 0.9800000190734863
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
   game rule `tnt_explosion_drop_decay` is true (default false, GameRules.txt 790–800);
   only
   `DESTROY_WITH_DECAY` gives the loot `EXPLOSION_RADIUS`, and without it
   `survives_explosion` holds without a draw and `explosion_decay` keeps every item
   (N4 brief 8.3; Clustine's `clustine_blocks::loot` already answers so). So TNT's drops
   are a rule's drops of the same blocks, with two differences: the loot has
   `THIS_ENTITY` (the TNT), so `entity_properties {"entity": "this"}` holds and **a chorus
   flower and a snow layer drop** (they drop nothing when broken by a rule), and the
   stacks are **merged across blocks** before they are laid (item 5). Block explosions
   (beds, respawn anchors) and mobs' decay by default (`block_explosion_drop_decay`,
   `mob_explosion_drop_decay` true), with `1/radius` per item. The loot parameters are
   built by `BlockBehaviour.onExplosionHit` (BlockBehaviour.txt 164–249, 2.6): `ORIGIN`
   the block's centre, `TOOL` empty, `BLOCK_ENTITY` and `THIS_ENTITY` (the TNT) when
   present, and `EXPLOSION_RADIUS` only with decay.
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
   `explosion(tnt, owner)`, whose type is `player_explosion` when both the TNT and its
   owner are there (an owner of any kind, not only a player) and `explosion` otherwise,
   with the TNT as the direct entity and the owner as the cause (DamageSources.txt
   543–555); a vehicle hurt by the
   explosion of a creative player's TNT is discarded without its item, and a frame,
   painting or cushion broken by it drops nothing, since their `hurtServer` reads the
   source's entity, the owner (N5 brief 3.14, section 6, 5.8). TNT lit by redstone or
   fire has no owner: the vehicle breaks with its item when its damage passes 40, and
   hanging things drop. Blocks drop either way (item 4).
8. **Creative players are not hurt but are knocked back** unless they fly: every
   player not a spectator and not flying in creative is put in `hitPlayers` with its
   push, which goes to that player in the explosion packet; the client applies it
   (**Client**). The server also adds it to the player's own motion (`push`), which
   Clustine does not simulate (the client moves the player).
9. **The packet goes to every player of the level within 64 blocks of the centre**
   (ServerLevel.txt 2640–2686: `distanceToSqr(center) < 4096`), not to the trackers of
   an entity or a chunk, each with its own knockback or none. In Clustine that is the
   edge's to fan out to players of any region (section 9).
10. **Block resistance is not in Clustine's data.** `Block.getExplosionResistance()` is
    the `explosionResistance` of the block's `Properties` (`strength(d, r)` sets it to
    `max(0, r)`, `strength(f)` to `f`, `instabreak()` to 0, default 0), and every ray
    point needs it with `FluidState.getExplosionResistance()` (the fluid's: 100 for
    water and lava, 0 for empty, from SteelMC's 26.3 `fluids.json`; the fluid classes
    are not dumped). Read from `Blocks`' static initialiser, 2.2 gives every value
    (stone 6, planks 3, glass 0.3, obsidian 1200, bedrock 3 600 000, water and lava
    blocks 100, TNT 0). `reports/blocks.json` has none; the datagen's `Extract.java`
    should write `getExplosionResistance()` per block beside the physics it already
    writes, which also catches a block class that changes its properties in its
    constructor (2.2), and 2.2's table is the check of that extract.

### 2.1 How an explosion starts: `ServerLevel.explode`

`Level.explode(source, damageSource, calculator, x, y, z, radius, fire, interaction)`
(Level.txt 1045–1062) adds the defaults: small particle `explosion` (30), large particle
`explosion_emitter` (29), block particles `DEFAULT_EXPLOSION_BLOCK_PARTICLES`, sound
`entity.generic.explode` (703). The default block particles (Level.txt 2162–2178): a
`WeightedList` of `ExplosionParticleInfo(poof, 0.5f, 1.0f)` and
`ExplosionParticleInfo(smoke, 1.0f, 1.0f)`, each added with the builder's default
weight, 1 (`WeightedList$Builder.add(E)` = `add(E, 1)`, WeightedList$Builder.txt
14–20). `ExplosionParticleInfo`'s two floats are **Inferred** to be a scaling and a
speed (the class is not in either dump).

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
e = new ServerExplosion(this, source, damageSource, calculator, center, radius, fire, blockInteraction)
count = e.explode()
particle = e.isSmall() ? smallParticle : largeParticle
for player in this.players:                                  // the level's list, in its order
    if player.distanceToSqr(center) < 4096.0:
        knock = Optional.ofNullable(e.getHitPlayers().get(player))
        player.connection.send(ClientboundExplodePacket(center, radius, count, knock,
            particle, sound, blockParticles, source == null || !source.isSilent()))
```

`getDestroyType(rule)` (2688–2700): the rule true → `DESTROY_WITH_DECAY`, false →
`DESTROY`. The switch map (`ServerLevel$1`, its static initialiser) numbers the cases
by the enum's ordinals as `Level$ExplosionInteraction` declares them, `NONE` 0, `BLOCK`
1, `MOB` 2, `TNT` 3, `TRIGGER` 4 (Level$ExplosionInteraction.txt 77–110), to the labels
1 to 5 in that order: the mapping above is the bytecode's.

Game rule defaults (`GameRules`' static initialiser, `registerBoolean(name, category,
default)`): `tnt_explodes` true (GameRules.txt 777–787, category misc),
`tnt_explosion_drop_decay` **false** (790–800, drops), `block_explosion_drop_decay` true
(478–), `mob_explosion_drop_decay` true (619–), `mob_griefing` true (624–), and
`block_drops` true (473–; `Block.popResource` lays nothing when it is off, Block.txt
1000, 1022). They agree with SteelMC's 26.3 extraction. All are in the 26.3 registry
`minecraft:game_rule` (`reports/registries.json`).

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

**The resistance of blocks.** `Block.getExplosionResistance()` returns the field
`explosionResistance` (Block.txt 1034–1038), copied from the block's
`BlockBehaviour$Properties`. In `Properties` (BlockBehaviour$Properties.txt):
`strength(d, r)` (403–409) = `destroyTime(d).explosionResistance(r)`; `strength(f)`
(419–426) = `strength(f, f)`; `instabreak()` (412–417) = `strength(0)`;
`explosionResistance(r)` (588–596) stores `Math.max(0.0f, r)`; the default is 0 (the
constructor, 70–139, sets no resistance); `ofFullCopy(block)` and `ofLegacyCopy(block)`
(147–, 190–) copy the other block's destroy time and resistance (stairs, slabs and
walls are made so from their full block). Every value below was read from `Blocks`'
static initialiser (Blocks.txt 6372–) and its helpers (`logProperties`,
`leavesProperties`, `buttonProperties`, `pistonProperties`, `shulkerBoxProperties`,
`registerStair`, `registerSlab`, `registerWall`, …, 1875–2142) by following each
block's `Properties` from `of()` or the copy to `register`, with a script; the coloured
and copper families are built in lambdas (`ColorCollection.registerBlocks`, the
weathering families), whose values were read from the lambdas' bodies and matched to
their families by sound and value (**Inferred** which lambda is which family where
two families share a value; the 1.21 values agree).

| Resistance (`float`) | Blocks |
|---|---|
| 0 | air, cave air, void air, fire and soul fire, every sapling, flower, crop, stem, mushroom, coral and coral fan (live and dead), grass, fern, bush, vine-like plants (kelp, cave vines, hanging roots, roots, sprouts, pale hanging moss), lily pad, sugar cane, sweet berry bush, torches (all, incl. copper and redstone), redstone wire, repeater, comparator, tripwire, tripwire hook, scaffolding, slime block, honey block, flower pots and every potted plant, decorated pot, end rod, frogspawn, moving piston, nether portal, structure void, **TNT**, bubble column, the infested blocks as `Blocks` builds them (but see below), dried ghast, leaf litter, pink petals, … (194 in all) |
| 0.1 | candle and the coloured candles, big dripleaf and its stem, moss block and moss carpet (and the pale ones), snow layer; carpets (all colours) |
| 0.2 | leaves (all), mushroom blocks and stem, daylight detector, glow lichen, sculk, sculk vein, snow block, straw bed, vine; beds (all colours) |
| 0.25 | powder snow, suspicious sand and gravel |
| 0.3 | glass, glass pane, tinted glass, glowstone, sea lantern, redstone lamp, froglights, bee nest; stained glass and stained glass panes (all colours) |
| 0.4 | cactus, chorus flower and plant, netherrack, crimson and warped nylium, ladder |
| 0.5 | dirt, coarse dirt, podzol, rooted dirt, mud, sand, red sand, soul sand, soul soil, ice, packed ice, frosted ice, magma block, hay bale, cake and candle cakes, brewing stand, every button and every pressure plate (wood, stone, polished blackstone, both weighted), lever, target, turtle egg, sniffer egg; concrete powder (all colours) |
| 0.6 | grass block, mycelium, farmland, gravel, clay, sponge, wet sponge, composter, beehive, honeycomb block |
| 0.65 | dirt path |
| 0.7 | rail, powered, detector and activator rails, mangrove roots, muddy mangrove roots |
| 0.75 | calcite (**Inferred**: and the infested blocks, below) |
| 0.8 | sandstone and red sandstone (plain, chiseled, cut, their stairs, slabs but cut, walls), quartz block, pillar, bricks, chiseled quartz, quartz stairs and slab, note block; wool, and the new wool stairs and slabs (all colours) |
| 1.0 | signs, wall signs, hanging signs and wall hanging signs (all woods), heads and skulls (all), pumpkin, carved pumpkin, jack o'lantern, melon, bamboo, bamboo sapling, dripstone block, nether and warped wart blocks, shroomlight; banners and wall banners (all colours) |
| 1.4 | glazed terracotta (all colours) |
| 1.5 | piston, sticky piston, piston head, bookshelf, chiseled bookshelf, amethyst block, budding amethyst, amethyst buds and cluster, sculk sensor, calibrated sculk sensor |
| 1.8 | concrete, and its new stairs and slabs (all colours) |
| 2.0 | logs, woods, stems, hyphae (stripped or not, all kinds), bamboo block, bone block, campfire and soul campfire, cauldrons (all four), shulker box and the coloured shulker boxes |
| 2.5 | chest, trapped chest, barrel, crafting table, cartography, fletching and smithing tables, loom, lectern, dried kelp block |
| 2.8 | blue ice |
| 3.0 | planks, wooden stairs, slabs, fences, gates, doors, trapdoors, shelves (all woods), the ores and deepslate ores (coal, copper, iron, gold, redstone, lapis, diamond, emerald), nether gold and quartz ores, lapis block, mud bricks (and their stairs, slab, wall), beacon, conduit, sculk catalyst and shrieker, cocoa, … (139 in all); **Inferred** (lambdas not resolved): copper doors and trapdoors (1.21: `strength(3.0f)`) |
| 3.5 | furnace, blast furnace, smoker, dispenser, dropper, crafter, lantern, soul lantern, lodestone, stonecutter; **Inferred**: the copper lanterns (a copy of the lantern in 1.21) |
| 4.0 | cobweb |
| 4.2 | terracotta, and the dyed terracotta (all colours); basalt, polished basalt, smooth basalt |
| 4.8 | hopper |
| 5.0 | iron door, iron trapdoor, bell, spawner |
| 6.0 | stone and its kinds (granite, diorite, andesite and the polished ones, smooth stone), cobblestone, mossy cobblestone, stone bricks and their kinds, bricks, deepslate and its kinds, tuff, blackstone, nether bricks, prismarine, purpur, resin bricks, the new cinnabar and sulfur blocks, smooth sandstone, coral blocks, the blocks of iron, gold, diamond, emerald, redstone, coal and the raw ores, iron bars, iron chain, jukebox, and every stairs, slab and wall of these (sandstone slabs too), … (189 in all); the copper blocks, cut copper, chiseled copper, copper grates, bulbs, golem statues and the cut copper stairs and slabs (3/6), copper bars and chains (5/6), every weathering stage and waxed; **Inferred** (lambdas not resolved): lightning rods 6 (1.21: `strength(3.0f, 6.0f)`) |
| 9.0 | end stone, end stone bricks and their stairs, slab and wall, dragon egg |
| 10.0 | creaking heart |
| 50.0 | trial spawner, vault |
| 100.0 | water, lava (the blocks; their fluids too) |
| 600.0 | ender chest |
| 1200.0 | obsidian, crying obsidian, ancient debris, netherite block, anvil (all three), enchanting table, respawn anchor, reinforced deepslate, heavy core |
| 3 600 000.0 | bedrock, end portal, end portal frame, end gateway, command blocks (all three), structure block, jigsaw, test block, test instance block |
| 3 600 000.8 | barrier, light |

Notes:
- The dyed terracotta's lambda is `strength(1.25f, 4.2f)`: hardness 1.25, resistance
  4.2, as plain terracotta.
- **The infested blocks** get no strength from `Blocks` (`Properties.of().mapColor(…)`,
  Blocks.txt 10512–10518); `InfestedBlock`'s constructor (not dumped) sets
  `destroyTime(host / 2)` and `explosionResistance(0.75f)` in 1.21 (**Inferred**, to be
  checked by the datagen extract). Any other block class whose constructor changes its
  properties is caught only by an extract of `getExplosionResistance()` from the live
  block, which is why the extract is the source and this table its check.
- Fluids: `FluidState.getExplosionResistance()` = `getType().getExplosionResistance()`
  (FluidState.txt 232–237); `WaterFluid`, `LavaFluid` and `EmptyFluid` are not dumped
  (100, 100, 0 by SteelMC's extraction and 1.21). The calculator takes `max(block,
  fluid)`, so **a waterlogged block resists 100** whatever the block.
- A ray loses `(r + 0.3f) · 0.3f` in `float` at each point: with `r` 0 a point costs
  `0.09f` plus the step's `0.22500001f`; with 1200 the ray ends at once. So a TNT can
  break a block of resistance up to about 17 at its centre (`5.2/0.3 − 0.3`), and
  nothing of 100 or more.

### 2.3 `explode()` (lines 849–885), in order

```
level.gameEvent(source, GameEvent.EXPLODE, center)     // section 8: game event 23
positions = calculateExplodedPositions()               // 2.4: 1352 level-random floats
hurtEntities()                                         // 2.5
if interactsWithBlocks(): interactWithBlocks(positions) // 2.6: the shuffle, the blocks, the drops
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
x0 = Mth.floor(center.x - (double)f - 1.0); x1 = Mth.floor(center.x + (double)f + 1.0)   // y0, y1, z0, z1 alike
for e in level.getEntities(source, AABB(x0, y0, z0, x1, y1, z1)):    // not the source; no spectators
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
    res = e is LivingEntity ? e.getAttributeValue(EXPLOSION_KNOCKBACK_RESISTANCE) : 0.0   // default 0, range 0..1 (Attributes.txt 249–260); every living entity has it (LivingEntity.createLivingAttributes, n5 LivingEntity.txt 435–)
    k = (1.0 - d) * (double) seen * (double) kb * (1.0 - res)
    push = dir.scale(k)
    e.pushFromExplosion(push)                                         // Entity: push(push): motion += push; needsSync (Entity.txt 5299–5328)
    if e.is(#redirectable_projectile) && e is Projectile:
        e.setOwner(damageSource.getEntity())                          // N9
    else if e is Player && !e.isSpectator() && (!e.isCreative() || !e.getAbilities().flying):
        hitPlayers.put(e, push)
    e.onExplosionHit(source)                                          // ServerPlayer: notes the impact (fall damage of wind charges)
```

- `getEntities(except, box)` (EntityGetter.txt 19–26) uses `NO_SPECTATORS`;
  `Level.getEntities(except, box, predicate)` (Level.txt 1261–) asks
  `LevelEntityGetter.get(box, consumer)` and then the ender dragon's parts. The order is
  `EntitySectionStorage.forEachAccessibleNonEmptySection` (EntitySectionStorage.txt
  37–148): the box widened by 2 in x and z and by 4 below (sections of 16), then section
  columns by **x ascending**, and within one x the existing sections in the order of
  their `SectionPos.asLong` key (z, then y, each as the key's unsigned bits, so 0, 1, …
  before the negative ones), only those whose y and z lie in range, non-empty and
  accessible; within a section the entities in the section's own order (`EntitySection`
  and `ClassInstanceMultiMap` are not dumped; **Inferred** insertion order, and the box
  test there, `AABB.intersects`, strict). The order matters only for which of two
  entities' deaths drops first; each entity is hurt and pushed once.
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
| item | `!shouldAffectBlocklikeEntities()`, else `Entity`'s false (ItemEntity.txt 787–797): an item is hit by TNT | `ItemEntity.hurtServer` (ItemEntity.txt 732–785): refused if `isInvulnerableToBase`; refused if `mob_griefing` is off and the cause is a `Mob`; refused if the stack's `damage_resistant` covers the source (`ItemStack.canBeHurtBy`, ItemStack.txt 2567–2583: the nether star's `#is_explosion`); else `markHurt`, `health = (int)((float) health − amount)` (`f2i`, towards zero; health starts at 5), the game event `entity_damage` with the cause, and at 0 or less `ItemStack.onDestroyed(item)` then `discard()`. **A destroyed shulker box item spills its contents**: `BlockItem.onDestroyed` (BlockItem.txt 404–419) empties the `container` component and drops its stacks (`ItemUtils.onContainerDestroyed`, not dumped; **Inferred** each as an item at the entity) — with no components in Clustine's stacks there is nothing to spill (section 0 item 5) |
| primed TNT | no | refuses (false); pushed from its feet |
| falling block | no (`Entity`'s, Entity.txt 10072–10075) | refuses: `hurtServer` only `markHurt`s unless invulnerable and answers false (FallingBlockEntity.txt 149–162); pushed |
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
Util.shuffle(positions, level.random)                    // n − 1 draws: nextInt(n), …, nextInt(2)
for pos in positions:
    level.getBlockState(pos).onExplosionHit(level, pos, this, (stack, p) -> addOrAppendStack(drops, stack, p))
for c in drops:
    Block.popResource(level, c.pos, c.stack)              // N4 brief 8.4: three level-random draws, then the item
```

`Util.shuffle(list, random)` (Util.txt 1957–1987): `for i = size; i > 1; i--: j =
random.nextInt(i); swap(list[i−1], list[j])`.

**`BlockBehaviour.onExplosionHit(state, level, pos, explosion, onDrop)`**
(BlockBehaviour.txt 164–249; `BlockStateBase.onExplosionHit`, BlockStateBase.txt 915–,
passes its state to the block's):

```
if state.isAir() || explosion.getBlockInteraction() == TRIGGER_BLOCK: return
block = state.getBlock()
byPlayer = explosion.getIndirectSourceEntity() instanceof Player
if block.dropFromExplosion(explosion):                     // Block's true (Block.txt 1208–1211); TNT's false
    blockEntity = state.hasBlockEntity() ? level.getBlockEntity(pos) : null
    params = LootParams.Builder(level)
        .withParameter(ORIGIN, Vec3.atCenterOf(pos)).withParameter(TOOL, ItemStack.EMPTY)
        .withOptionalParameter(BLOCK_ENTITY, blockEntity)
        .withOptionalParameter(THIS_ENTITY, explosion.getDirectSourceEntity())
    if explosion.getBlockInteraction() == DESTROY_WITH_DECAY:
        params.withParameter(EXPLOSION_RADIUS, explosion.radius())    // a Float
    state.spawnAfterBreak(level, pos, ItemStack.EMPTY, byPlayer)  // ores' experience, infested blocks' silverfish
    state.getDrops(params).forEach(stack -> onDrop.accept(stack, pos))
level.setBlockAndUpdate(pos, AIR)                          // flag 3
block.wasExploded(level, pos, explosion)                   // Block's does nothing (Block.txt 1040–1042)
```

This is the 1.21 rule the first reading assumed, now read. The order within one block
is: experience and other `spawnAfterBreak` effects, the loot (collected, not laid), the
set to air with its updates and, inside the set, the container's spill (below), then
`wasExploded`. The loot reads the block entity before the set removes it.

- Blocks that override `onExplosionHit` for `canTriggerBlocks` (buttons, levers, bells,
  doors and the like, for wind charges) do nothing different for TNT (**Inferred**:
  their classes are not dumped; `Block`, `BlockBehaviour` and `TntBlock` are, and only
  `BlockBehaviour` defines it).
- The set is flag 3 (neighbours and clients): each block's removal updates its
  neighbours and their shapes before the next block of the shuffled list is handled, so
  sand above falls, water flows in, torches pop (their own drops, by `popResource`, at
  once and not merged), and a block that a neighbour's update has already removed is
  air when its turn comes (skipped). Clustine's updates do this through `updates.rs`.
- A block with a container (chest, barrel, dispenser, hopper, …) spills its contents as
  it is set to air: `LevelChunk.setBlockState` (LevelChunk.txt 488–; the test at lines
  627–685) calls `BlockEntity.preRemoveSideEffects` when the
  block changes to another block, the old one has a block entity that the new state does
  not keep, the level is the server's and the flags lack 256
  (`UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS`), then removes the block entity; the base
  `preRemoveSideEffects` is `Containers.dropContents(level, pos, container)` for a
  `Container` (BlockEntity.txt 557–575; N5 brief 3.18 for the draws: per slot three
  `nextDouble` of the level's random, then per part `nextInt(21)` and
  `triangle(0, b)`, `triangle(0.2, b)`, `triangle(0, b)` with `b =
  0.11485000171139836` (Containers.txt 124–145), each `a + b·(nextDouble −
  nextDouble)`, two draws (RandomSource.txt 77–88)). These items are laid at once, before
  the collected drops. A shulker box overrides it with nothing (ShulkerBoxBlockEntity.txt
  342–344) and keeps its contents in its item (`copy_components`), which Clustine's loot
  cannot carry (`clustine_blocks::loot`): **an exploded shulker box's contents would be
  lost** unless N6 gives items their components (section 3's matter).
- `wasExploded` is `TntBlock`'s (1.2) for TNT; `Block`'s does nothing; other blocks'
  overrides are in classes not dumped (**Inferred**: none of the blocks a creative
  player meets does anything there).
- **`addOrAppendStack(drops, stack, pos)`** (lines 887–915): for each collector in order,
  `tryMerge(stack)`; stop as soon as `stack` is empty; if any is left, a new collector
  `(pos, stack)` at the end.
- **`StackCollector.tryMerge(other)`** (StackCollector.txt 19–33): if
  `ItemEntity.areMergable(this.stack, other)` (ItemEntity.txt 602–617: `other.count +
  this.count ≤ other.maxStackSize`, then `ItemStack.isSameItemSameComponents`),
  `this.stack = ItemEntity.merge(this.stack, other, 16)` (619–640): `moved =
  min(min(this.max, 16) − this.count, other.count)`, `this.stack` becomes a copy with
  `count + moved`, `other` shrinks by `moved`. A
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
`Level.updateNeighbourForOutputSignal`, `CollisionGetter.noCollision`,
`Mth.lerpDiscrete`,
`ContainerEntity`, `MinecartChest`, `AbstractChestBoat`. Ids are from
`generated/reports/registries.json`.

### 3.0 Findings of this section

1. **A click on a chest, barrel, shulker box, dispenser, dropper, hopper or crafter is
   the block's `useWithoutItem`**, reached from `ServerPlayerGameMode.useItemOn`
   (ServerPlayerGameMode.txt 1029–1171) whenever the player is not sneaking with
   something in a hand: `BlockState.useItemOn` answers `TRY_WITH_EMPTY_HAND` for these
   blocks (none overrides `useItemOn`, and `BlockBehaviour.useItemOn` answers
   `TRY_WITH_EMPTY_HAND`, BlockBehaviour.txt 255–258), and the main hand then runs
   `useWithoutItem`. Every one answers `SUCCESS` on the server
   whatever happens, so **the click is consumed even when nothing opens** (a blocked
   chest,
   a shulker box that cannot open, a locked container): no block is placed against it.
   Today `placement.rs` (`is_used`, `use_block`) already treats these blocks as used and
   does nothing; N6 replaces that nothing with opening a menu.
2. **Opening is `ServerPlayer.openMenu(provider)`** (ServerPlayer.txt 3191–3248, N5
dump),
   and its order matters to the packets: close the open menu if any (not the inventory)
   → `containerCounter = containerCounter % 100 + 1` (3179–3189; 1 to 100, advanced even
   when nothing opens) → `provider.createMenu(counter, inventory, player)`, whose menu
   constructor calls `container.startOpen(player)` (the openers' count, the sound, the
   block event, the game event) → if null: a spectator is told
   `container.spectatorCantOpen` (red, overlay), and nothing else → else
   `ClientboundOpenScreenPacket(containerId, menu type, provider.getDisplayName())` →
   `initMenu(menu)` (the synchroniser's first full send, 3.10 on) → `containerMenu =
   menu`.
3. **The menus and titles** (`minecraft:menu` protocol ids): chest, trapped chest, copper
   chests and barrel `generic_9x3` (2); a double chest `generic_9x6` (5); shulker boxes
   `shulker_box` (20); dispenser and dropper `generic_3x3` (6); hopper `hopper` (16);
   crafter `crafter_3x3` (7). The title is the custom name if set, else the translatable
   `container.chest`, `container.chestDouble`, `container.barrel`,
   `container.shulkerBox`,
   `container.dispenser`, `container.dropper`, `container.hopper`, `container.crafter`. A
   double chest takes the first half's custom name, else the second's, else
   `container.chestDouble` (3.3).
4. **A double chest is a `CompoundContainer(first, second)` where `first` is the half
   whose `type` is `right`**: slots 0–26 are that half's, 27–53 the `left` half's.
   `ChestBlock.getBlockType`: `right` → `FIRST`, `left` → `SECOND` (ChestBlock.txt
   56–75).
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
    every block entity that is a `Container` when its block is removed: `LevelChunk
    .setBlockState` (LevelChunk.txt 636–688) calls it on the server when the block
    changes
    to another block, the old state has a block entity that the new state does not keep
    (`shouldChangedStateKeepBlockEntity`), and the flags lack 256
    (`UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS`), then removes the block entity; whoever
    breaks
    it: the N4 rule "a creative player's break drops nothing" covers the
    block's own loot only. Each slot drops as items of 10 to 30 at a time, each stack
    with one `nextInt(21)` and three `triangle`s of two `nextDouble` each (3.6, section
    10). A chest with an unopened loot table
    unpacks it first (`getItem` unpacks), so **breaking a structure's chest spills its
    loot**. The shulker box overrides `preRemoveSideEffects` to nothing (its contents go
    in its item), and a creative player breaking a non-empty shulker box gets the box as
    an item with its contents (`playerWillDestroy`, 3.5).
11. **Loot tables are unpacked at the first touch, not at generation.** Chests, barrels,
    dispensers, hoppers and shulker boxes placed by structures keep `LootTable` (and
    `LootTableSeed`) in their data, which Clustine's structures already write
    (`crates/clustine-structures/src/resave.rs`). Opening one, reading it through a
    comparator (`getItem`), a hopper pulling, or breaking it, runs `unpackLootTable`:
    the table filled into the container with the seed (`RandomizableContainer
    .unpackLootTable`, RandomizableContainer.txt 102–168; how `LootTable.fill` turns the
    seed into a random is not read). Clustine has
    only block loot tables (`clustine-data`'s `loot_tables.rs`: 1201 `blocks/…`, no
    `chests/…`); the 31 `chests/…`, `barrels/…` and `dispensers/…` tables are in
    `generated/data/minecraft/loot_table/`. A spectator cannot open a container with a
    loot table still set.
12. **Locks** (`BaseContainerBlockEntity.canOpen`: `lockKey.canUnlock(player)`): a
    container with a `lock` component opens only for a player whose main hand matches
    its predicate, **or for any spectator** (`LockCode.canUnlock`, LockCode.txt 40–54:
    `isSpectator() || predicate.test(mainHand)`; an absent lock is `NO_LOCK`, an empty
    item predicate, which any stack passes, **Inferred**: `ItemPredicate` is not read). A
    refused player is sent
    `container.isLocked` with the container's name as an overlay message, and
    `block.chest.locked` (348) plays at the block's centre at volume and pitch 1. Only
    `/data` or an item with the component makes a lock; a creative player meets one in a
    map that has one.
13. **Copper chests, trapped chests and the ender chest are chests of their own
    classes** (3.3): the copper chests (eight blocks, `CopperChestBlock` and
    `WeatheringCopperChestBlock`) share the `chest` block entity type (BlockEntityTypes
    .txt 171–178), sound by their oxidation (`block.copper_chest*.open/close`; the
    exposed ones the plain copper sounds) and **join any copper chest**, both halves
    becoming the less oxidised block (N1's placement does not do that yet); the trapped
    chest (`trapped_chest` block entity, 2) is a signal source of its half's openers'
    count, 0 to 15, strongly downwards, and updates its neighbours and those below it
    when the count changes; the ender chest opens the player's own ender inventory (27
    slots, `generic_9x3`, `container.enderchest`, blocked by a conductor above), which
    N2's player data does not keep yet, and counts openers through its own block entity
    (`EnderChestBlockEntity`, not dumped).
14. **No container block sends `block_entity_data`.** `BlockEntity.getUpdatePacket` is
    null and `getUpdateTag` an empty compound (BlockEntity.txt 525–535), and none of
    these
    block entities overrides either: a client learns nothing of a container's contents
    or name except through a menu. The chunk packet lists them with an empty tag.

#### Findings of 3.10 to 3.21 (menus and clicks)

Items 15 to 26 are the findings of 3.10 to 3.21 (the menus, the clicks and the
synchroniser); items 1 to 14 above are those of the container blocks.

15. **`ClickType` is now `ContainerInput`** (`net.minecraft.world.inventory.ContainerInput`,
   ContainerInput.txt): `PICKUP` 0, `QUICK_MOVE` 1, `SWAP` 2, `CLONE` 3, `THROW` 4,
   `QUICK_CRAFT` 5, `PICKUP_ALL` 6 (each constructed with its id, `static {}` 91–137). Its
   `STREAM_CODEC` is `ByteBufCodecs.idMapper(BY_ID, id)`, a VarInt (`ByteBufCodecs$31`), and
   `BY_ID` is `ByIdMap.continuous(id, values, ZERO)`: **an id outside 0–6 decodes as
   `PICKUP`**, not as an error (**Inferred** from the strategy's name: `ByIdMap` is not
   dumped). `ClickAction` (ClickAction.txt) is `PRIMARY` (ordinal 0) and `SECONDARY` (1),
   chosen from the button (0 or 1) by `getClickAction`; it has no wire form (3.14).
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
   the carried stack does not (the synchroniser, `ServerPlayer$1.txt` 29–126). A client
   that predicted right is sent nothing (3.15, 3.16). A component's hash is the CRC32C of
   the component's value encoded through `HashOps` (a tagged byte stream), as an `int`
   (3.16).
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
   its player is 8.5 blocks away, 9 in creative: `block_interaction_range` is 4.5
   (Attributes.txt 199–209, a ranged attribute of 0 to 64), and a creative player has the
   transient modifier `creative_mode_block_range` of +0.5 (`ADD_VALUE`, ServerPlayer.txt
   5962–5973, put on and taken off by `updatePlayerAttributes`, 1022–1060). A click in
   between is refused silently (item 16).
23. **The crafter's slot toggles are not clicks**: `container_slot_state_changed`
   (serverbound 20) reaches `CrafterBlockEntity.setSlotState` (SGPLI 2824–2869) with only a
   spectator check and a container id check: no `stillValid`, no slot range check here
   (section 7). `container_button_click` (17) calls `clickMenuButton`, which is `false` for
   every N6 menu (AbstractContainerMenu.txt 810–813): ignore it.
24. **`set_creative_mode_slot` reaches slots 1 to 45 of the inventory menu** (SGPLI
    6226–6329), the crafting grid's 1 to 4 included, even while another menu is open; the
    edge keeps only 5 to 45 today (`Inventory::from_container`). The 2×2 grid's items live
    on the server in the game and are put back into the inventory (or dropped) only when
    the inventory menu is `removed`, which is at the player's removal (`Player.remove`,
    n5 Player.txt 3095–3112; `InventoryMenu.removed`, InventoryMenu.txt 169–188). A small
    named difference, or the grid kept (3.21).
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
| trapped chest (`TrappedChestBlock`) | inherited | `TrappedChestBlockEntity` (`trapped_chest`, 2) | as chest | as chest | as chest | as chest |
| ender chest (`EnderChestBlock`) | EnderChestBlock.txt 69–127 | `EnderChestBlockEntity` (`ender_chest`; not dumped) | 27, the player's own | `generic_9x3` (2) | `container.enderchest` | no conductor above |
| barrel (`BarrelBlock`) | BarrelBlock.txt 29–59 | `BarrelBlockEntity` (`barrel`, 26) | 27 | `generic_9x3` (2) | `container.barrel` | lock passes |
| shulker boxes, 17 (`ShulkerBoxBlock`) | ShulkerBoxBlock.txt 50–86 | `ShulkerBoxBlockEntity` (`shulker_box`, 24) | 27 | `shulker_box` (20) | `container.shulkerBox` | `canOpen` (3.5) and lock |
| dispenser (`DispenserBlock`) | DispenserBlock.txt 61–89 | `DispenserBlockEntity` (`dispenser`, 5) | 9 | `generic_3x3` (6) | `container.dispenser` | lock passes |
| dropper (`DropperBlock` extends `DispenserBlock`) | inherited | `DropperBlockEntity` (`dropper`, 6) | 9 | `generic_3x3` (6) | `container.dropper` | lock passes |
| hopper (`HopperBlock`) | HopperBlock.txt 189–212 | `HopperBlockEntity` (`hopper`, 18) | 5 | `hopper` (16) | `container.hopper` | lock passes |
| crafter (`CrafterBlock`) | CrafterBlock.txt 271–291 | `CrafterBlockEntity` (`crafter`, 42) | 9 | `crafter_3x3` (7) | `container.crafter` | lock passes |

Each `useWithoutItem`, on the server only:

```
chest:   provider = getMenuProvider(state, level, pos)            // 3.3; null when blocked or no block entity
         if provider != null:
             player.openMenu(provider)
             player.awardStat(getOpenChestStat())                 // OPEN_CHEST; the trapped chest's TRIGGER_TRAPPED_CHEST
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
ender:   inv = player.getEnderChestInventory(); be = level.getBlockEntity(pos)
         if inv == null || be is not EnderChestBlockEntity: return SUCCESS
         if level.getBlockState(pos.above()).isRedstoneConductor(level, pos.above()): return SUCCESS
         if server: inv.setActiveChest(be)
                    player.openMenu(SimpleMenuProvider((id, i, p) -> ChestMenu.threeRows(id, i, inv),
                                                       "container.enderchest"))
                    awardStat(OPEN_ENDERCHEST); angerNearbyPiglins(…, true)
         return SUCCESS
```

`PlayerEnderChestContainer` (PlayerEnderChestContainer.txt, 141 lines) is a
`SimpleContainer(27)` kept with the player; `startOpen`/`stopOpen` pass to the active
chest's block entity, which counts openers and moves its lid as a chest does
(`EnderChestBlock.tick` runs its `recheckOpen`, 331–345; the block entity is not
dumped), and `stillValid` is the active chest's `stillValid` (94–110). Its contents are
saved with the player (`fromSlots`/`storeAsSlots`, the `ItemStackWithSlot` list), so the
ender chest needs a field in N2's player data.

Statistics and piglins are not Clustine's yet and can be left out (statistics are
named in `ServerPlayer` data no step keeps).

**Spectators** take another path (ServerPlayerGameMode.txt 1047–1062): a spectator's
click on any block asks `state.getMenuProvider(level, pos)` and, if not null, opens it
and answers `CONSUME`, without the block's `useWithoutItem` (no statistic, no piglins).
For a chest that is the combiner's provider (3.3); for the others `BaseEntityBlock`'s,
the block entity when it is a `MenuProvider`, else null (BaseEntityBlock.txt 34–50).
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
chests override it: any block of the tag `copper_chests` that has `type`,
CopperChestBlock.txt 50–63; see below).

**The combiner** (`combine(state, level, pos, ignoreBlocked)`, 442–463):
`DoubleBlockCombiner.combineWithNeigbour(blockEntityType, ChestBlock::getBlockType,
ChestBlock::getConnectedDirection, FACING, state, level, pos, blockedTest)`, the test
being always false when `ignoreBlocked` and `isChestBlockedAt` otherwise (**Inferred**
which `invokedynamic` is which: `lambda$combine$0` returns false, lines 693–697, and is
the first; no `javap -v` was read). `DoubleBlockCombiner.combineWithNeigbour`
(DoubleBlockCombiner.txt 9–124):

```
be = level's block entity at pos of the chest type;  none → NONE
if blockedTest(level, pos) → NONE
kind = getBlockType(state)                 // SINGLE, FIRST (right), SECOND (left)
if kind == SINGLE → SINGLE(be)
partner = pos.relative(getConnectedDirection(state)); ps = its state
if ps.is(state.getBlock()) && getBlockType(ps) != SINGLE && != kind
        && ps.facing == state.facing:
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
there has a `ChestBlockEntity`, else 0 (the trapped chest's signal);
`swapContents` (294–306) swaps two chests' lists (used by nothing read). Its counter
(`ChestBlockEntity$1`): `onOpen`/`onClose` play the block's `getOpenChestSound`/
`getCloseChestSound` (the constructor's, so the copper chests' own) through `playSound`;
`openerCountChanged` → `signalOpenCount`; `isOwnContainer(player)`:
`player.containerMenu`
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
unaffected and exposed ones the plain copper sounds (`CopperChestBlock.getHingeSound`,
CopperChestBlock.txt 20–48, switched by `$1`: weathered and oxidized have their own,
everything else the plain ones; Blocks.txt 2419–2448 passes `getHingeSound(state, true)`
as the open sound and `false` as the close); `block.chest.locked` (348).

**Copper chests joining** (CopperChestBlock.txt 50–193). `chestCanConnectTo(other)` is
`other.is(#copper_chests) && other.hasProperty(type)`, so any two copper chests, of any
oxidation, waxed or not, join. Placing one (`getStateForPlacement` →
`getLeastOxidizedChestOfConnectedBlocks`, 65–168): when `ChestBlock`'s placement made
it a half and the partner is a copper chest, both are compared unwaxed if exactly one is
waxed (`unwaxBlock`), and the placed chest becomes the block of lower `weatherState`
ordinal (the placed one on a tie), keeping its own properties
(`withPropertiesOf`). `updateShape` (156–193): after `ChestBlock.updateShape`, if the
neighbour in the update's direction is a copper chest and the result is a half whose
connected direction is that direction, the chest becomes the neighbour's block with its
own properties. So the two halves of a copper double chest are always the same block,
which `DoubleBlockCombiner`'s `is(state.getBlock())` needs; N1's placement and shape
updates join only the same block and have to learn this.

**The trapped chest** (TrappedChestBlock.txt, 62 lines; TrappedChestBlockEntity.txt,
48): `isSignalSource` true; `ownSignal` (the weak signal on every side, since
`BlockBehaviour.getSignal` is `ownSignal`, BlockBehaviour.txt 575–587) =
`Mth.clamp(ChestBlockEntity.getOpenCount(level, pos), 0, 15)`; `getDirectSignal` = that
signal when the direction asked is `UP` (the block below is strongly powered), else 0;
its stat is `TRIGGER_TRAPPED_CHEST`; the sounds and title are the chest's. Its entity's
`signalOpenCount(level, pos, state, old, new)` runs the chest's (the block event), then,
when `old != new`, `orientation = ExperimentalRedstoneUtils.initialOrientation(level,
facing.getOpposite(), UP)` and `level.updateNeighborsAt(pos, block, orientation)`, then
`level.updateNeighborsAt(pos.below(), block, orientation)`. Each half counts its own
openers, and a double chest's menu opens both, so both halves give the signal. It is the
one
container block whose openers are redstone, so the one that needs the openers' count
in the simulation for the circuit's sake.

### 3.4 The openers' count (ContainerOpenersCounter.txt, 274 lines)

State: `openCount` (int) and `maxInteractionRange` (double), neither saved (a chest
loaded
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
  `block_event` every 5 ticks while the chest is open: `ServerLevel.runBlockEvents`
  (ServerLevel.txt 2717–2785) sends `block_event` to every player of the dimension
  within 64 blocks of the position's corner whenever `doBlockEvent` (2786–2810: the
  block still there, then `triggerEvent`) answers true, which the chest's does for event
  1. Block events are queued in a linked set (`blockEvent`, 2702–2716), so the same
  position, block and parameters queued twice in a tick run once; an event at a
  position that does not tick (`shouldTickBlocksAt`) is put back for the next tick.
- `getContainerInteractionRange` is the player's `blockInteractionRange()` (Player.txt
  4815–4819), the attribute `block_interaction_range`: 4.5 (Attributes.txt 206–231,
  range 0 to 64), and 5.0 in creative (`creative_mode_block_range`, +0.5 `ADD_VALUE`,
  added by `ServerPlayer.updatePlayerAttributes` while creative, ServerPlayer.txt
  1022–1040, 5962–5973). The recheck's box is the block's
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
updateBlockState(state, open): level.setBlockAndUpdate(worldPosition, state.setValue(OPEN, open))   // flags 3
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
box = Shulker.getProgressDeltaAabb(1.0f, state.facing, 0.0f, 0.5f, Vec3.atBottomCenterOf(pos)).deflate(1.0E-6)
return level.noCollision(box)
```

`Shulker.getProgressDeltaAabb` is not read. **Inferred** from the game's source: the
unit box at the block, stretched in `facing` from progress 0 to 0.5 and kept to the
part beyond the block, so the half block just beyond the facing face (for `up`:
`x..x+1, y+1..y+1.5, z..z+1`). `noCollision(AABB)` (CollisionGetter.txt 45–92) is no
block
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
box = Shulker.getProgressDeltaAabb(1.0f, dir, progressOld, progress, Vec3.atBottomCenterOf(pos))
for e in level.getEntities((Entity) null, box):            // getEntities(Entity, AABB): every entity, no predicate
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
.getBoundingBox(state))` where `getBoundingBox` (192–213) is
`Shulker.getProgressAabb(1.0f,
facing, 0.5f * getProgress(1.0f), Vec3(0.5, 0, 0.5))` (the block grows by up to half a
block towards `facing`), and `Shapes.block()` without a block entity; the collision
shape follows it (`BlockBehaviour.getCollisionShape`, BlockBehaviour.txt 474–485, is
`getShape` for a block with collision, and `ShulkerBoxBlock` does not override it).
`getBlockSupportShape` (241–266) is the full block when closed and, while not closed,
`SHAPES_OPEN_SUPPORT.get(facing.getOpposite())` (a 16 × 16 × 1 slab against the face
opposite the lid: `Block.boxZ(16, 0, 1)` is `box(0, 0, 0, 16, 16, 1)`, Block.txt
257–302, rotated): what is attached to
the box's sides loses support when it opens (`doNeighborUpdates`).
`propagatesSkylightDown`
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
block entity; `getDrops` (208–231) also offers a dynamic drop `contents` (each slot)
which
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
and `canTakeItem` true; `countItem`, `hasAnyOf`, `hasAnyMatching` over the slots in
order.
`stillValidBlockEntity(be, player)` (116–148) with the buffer `4.0f`:

```
level = be.getLevel(); pos = be.getBlockPos()
if level == null: false
if level.getBlockEntity(pos) != be: false               // the block was replaced or its chunk unloaded
player.isWithinBlockInteractionRange(pos, 4.0)          // Player.txt 4883–4905:
    // new AABB(pos).distanceToSqr(player.getEyePosition()) < (blockInteractionRange() + 4.0)^2
```

So a menu over a block stays valid while the player's eyes are within 8.5 (9 in
creative) of the block's box. Who asks and when (every tick, closing the menu) is 3.10
on.

**`CompoundContainer`** (CompoundContainer.txt, 245 lines): size the sum; slot `i <
size1`
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
removeItem(i, n): r = ContainerHelper.removeItem(items, i, n)   // items[i].split(n), EMPTY when i out of range, empty or n <= 0
                  if !r.isEmpty(): setChanged()
removeItemNoUpdate(i): ContainerHelper.takeItem(items, i)       // items[i], replaced by EMPTY; no setChanged
setItem(i, s): items[i] = s; s.limitSize(getMaxStackSize(s)); setChanged()
stillValid(player): Container.stillValidBlockEntity(this, player)
clearContent: items.clear()                                       // no setChanged
getName: custom name or getDefaultName(); getDisplayName = getName
```

`RandomizableContainerBlockEntity` (lines 45–94) makes `isEmpty`, `getItem`,
`removeItem`, `removeItemNoUpdate` and `setItem` unpack the loot table first
(`unpackLootTable(null)`); so does anything that reads a slot (a comparator, a hopper,
breaking). `unpackLootTable(player)` is the `RandomizableContainer` interface's default
(RandomizableContainer.txt 102–168):

```
level = getLevel(); pos = getBlockPos(); key = getLootTable()
if key != null && level != null && level.getServer() != null:
    table = server.reloadableRegistries().getLootTable(key)
    if player is ServerPlayer: CriteriaTriggers.GENERATE_LOOT.trigger(player, key)
    setLootTable(null)                                  // a plain field write: no setChanged here
    params = LootParams.Builder(level).withParameter(ORIGIN, Vec3.atCenterOf(pos))
    if player != null: params.withLuck(player.getLuck()).withParameter(THIS_ENTITY, player)
    table.fill(this, params.create(LootContextParamSets.CHEST), getLootTableSeed())
```

The rolled stacks reach the slots through `setItem`, which calls `setChanged` per slot
(**Inferred**: `LootTable.fill` is not read). `tryLoadLootTable` (48–73) reads
`LootTable` (a loot table key) and `LootTableSeed` (default 0) and answers whether a
table was set; `trySaveLootTable` (74–101) writes `LootTable` when set and
`LootTableSeed` only when not 0, answering whether it wrote; `setBlockEntityLootTable`
(29–47), used by structures, sets the table with a seed of `random.nextLong()`.

(The same shape as `ContainerEntity.unpackChestVehicleLootTable`, read: ContainerEntity
.txt 125–181, with the entity's position as the origin.) `LootTable.fill` (not read)
takes
its random from the seed (**Inferred**, not read: the level's random when the seed is
0, else a fresh random of that seed) and spreads the rolled stacks over the empty slots
at random
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
`setDeltaMovement` then replaces: ItemEntity.txt 33–98, two `nextFloat` (bob, yaw) and
two `nextDouble` (the x and z motion), all of **the item's own random**, not the
level's). The
repository has this already for container vehicles (`region/vehicles.rs`,
`drop_contents`). `updateNeighboursAfterDestroy(state, level, pos)` (153–160) is
`level.updateNeighbourForOutputSignal(pos, state.getBlock())`, called by each of these
blocks' `affectNeighborsAfterRemoval` (ChestBlock.txt 372–378, BarrelBlock.txt 61–67,
ShulkerBoxBlock.txt 233–239, DispenserBlock.txt 296–303, HopperBlock.txt 253–260,
CrafterBlock.txt 263–270).

### 3.7 What is saved, and what the client is sent

Each block entity's data, as the game saves it (`saveWithoutMetadata` adds
`components`, BlockEntity.txt 273–284):

| Key | Written by | When |
|---|---|---|
| `lock` | `LockCode.addToTag` (BaseContainerBlockEntity.txt 35–50; LockCode.txt 28–39: the item predicate's codec) | when not `NO_LOCK` |
| `CustomName` | `storeNullable("CustomName", ComponentSerialization.CODEC, name)` | when named |
| `LootTable` | `trySaveLootTable` (RandomizableContainer.txt 74–101) | while a table is set |
| `LootTableSeed` (long) | `trySaveLootTable` | while a table is set and the seed is not 0 |
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
entity's display name as the title (`Entity.getDisplayName`, Entity.txt 9517–9527: the
team-formatted `getName`, which is the custom name, stripped of click actions, or the
type's description, 8424–8444), `createMenu` (a chest cart and a chest boat `ChestMenu
.threeRows`, a hopper cart `HopperMenu`; `null` for a spectator while a loot table is
set,
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
  (the openers' count and the lid, 3.1–3.9) before the open screen packet is sent; but the
  lid's block event is only queued there (`ServerLevel.blockEvent`, ServerLevel.txt
  2702–2716, into an `ObjectLinkedOpenHashSet`, so the same event twice in a tick is one)
  and sent when the level runs its block events (`runBlockEvents`, 2717–2785, called from
  `ServerLevel.tick` at 575), to every player within 64 blocks of the block. So
  `open_screen` and the content go first, at once, and the lid's `block_event` later, in
  the level's next block-event phase (**Inferred** only for where packet handling falls
  against the level's tick, which is `MinecraftServer`'s, not dumped).
- `initMenu(menu)` (858–868): `menu.addSlotListener(containerListener)` (which calls
  `broadcastChanges()` at once, 336–352, with no synchroniser yet, so it only fills
  `lastSlots` and tells the listener: `ServerPlayer$2.slotChanged`, ServerPlayer$2.txt,
  which triggers the advancement criterion `INVENTORY_CHANGED` when the slot is not a
  `ResultSlot` and its container is the player's inventory, and nothing else), then
  `menu.setSynchronizer(containerSynchronizer)` (353–369): the remote carried slot and every
  remote slot become `synchronizer.createSlot()`, and `sendAllDataToRemote()` (371–452):
  - every slot's stack is copied into a list and forced into its remote slot;
  - the carried stack is forced into the remote carried slot;
  - every data slot's value is copied into `remoteDataSlots`;
  - `synchronizer.sendInitialData(menu, items, carried.copy(), remoteDataSlots)`.
- What `sendInitialData` sends (ServerPlayer$1.txt 29–60):
  `ClientboundContainerSetContentPacket(containerId, menu.incrementStateId(), items,
  carried)`, then for each data slot `i` in order `ClientboundContainerSetDataPacket(
  containerId, i, value)` (`broadcastDataValue`, 98–113).
- `initInventoryMenu()` (870–876) does the same for `inventoryMenu` (container id 0) at
  join: the content the edge sends at join today (`fanout.rs`, `SetContainerContent` with
  window 0, state id 0) is that packet; the game's state id there is 1 (`sendInitialData`
  calls `incrementStateId` on a menu whose state id is 0). The client takes any state id;
  the edge's 0 is fine as long as later packets count from it.

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
  18 + row*18); inventory at (8, 84). `ShulkerBoxSlot.mayPlace(stack)` is
  `stack.getItem().canFitInsideContainerItems()` (ShulkerBoxSlot.txt): true by `Item`'s
  default (n5 Item.txt 590–593), false for a `BlockItem` whose block is a
  `ShulkerBoxBlock` (BlockItem.txt 393–402). So only shulker boxes are refused among the
  classes read (whether `BundleItem` overrides it is not read). Clicks, drags, swaps and
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
  A `CrafterSlot` (CrafterSlot.txt) takes a stack only while enabled
  (`!menu.isSlotDisabled(index) && super.mayPlace(stack)`), and its `setChanged` also
  calls `menu.slotsChanged(container)`. The result slot is a `NonInteractiveResultSlot`
  (NonInteractiveResultSlot.txt), which refuses everything: `mayPickup`, `mayPlace` and
  `allowModification` false, `tryRemove` empty, `safeTake` and `remove` `EMPTY`,
  `safeInsert` gives the stack back, `isFake` true.
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
if !(prev == 1 && quickcraftStatus == 2) && prev != quickcraftStatus: resetQuickCraft(); return
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
- `canDragTo(slot)` (2218–2221): true (result slots' menus override; none of N6's do:
  `CrafterMenu` extends `AbstractContainerMenu` directly and does not override it).
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
the carried stack). `ItemStack.overrideStackedOnOther` and `overrideOtherStackedOnMe`
(ItemStack.txt 938–960) ask the item, whose default is false (n5 Item.txt 344–352); only
the bundle overrides them among items (`BundleItem` is not dumped), so with no bundles in
N6 it is false.

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
if !stack.isEmpty():                                     // pass 2: the first empty slot that takes it
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
player.drop(t, true, PREDICTED); player.handleCreativeModeItemDrop(t)   // the latter does nothing
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
if c.isEmpty() || (s.hasItem() && s.mayPickup(player)): return   // the first click picked it up
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
false (NonInteractiveResultSlot.txt), which stops `QUICK_MOVE` before it.

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

- **Remote slots** (RemoteSlot.txt, RemoteSlot$1.txt, RemoteSlot$Synchronized.txt). A
  `Synchronized` slot holds a `remoteStack` and a `remoteHash`, both null at first, and its
  hash generator:
  - `force(stack)` (24–34): `remoteStack = stack.copy()`, `remoteHash = null`;
  - `receive(hashed)` (35–44): `remoteStack = null`, `remoteHash = hashed`;
  - `matches(stack)` (45–73): if `remoteStack` is set, `ItemStack.matches(remoteStack,
    stack)`; else if `remoteHash` is set and `remoteHash.matches(stack, hasher)`, **it
    then keeps `remoteStack = stack.copy()`** (so the next comparisons are by stack) and
    answers true; else false. A slot never forced nor told anything matches nothing;
  - `copyFrom(other)` (74–85): takes the other's `remoteStack` and `remoteHash` (the same
    objects), used by `transferState`.
  `PLACEHOLDER` (`RemoteSlot$1`) matches everything and ignores `force` and `receive`; it
  is every remote slot until a synchroniser is set.
- What the synchroniser sends (`ServerPlayer$1`, ServerPlayer$1.txt, one per player):
  - `sendInitialData` (29–60): `container_set_content(containerId,
    menu.incrementStateId(), items, carried)`, then one `container_set_data(containerId,
    i, value)` per data slot;
  - `sendSlotChange` (61–77): `container_set_slot(containerId, menu.incrementStateId(),
    slot, stack)`;
  - `sendCarriedChange` (78–89): `set_cursor_item(stack)` (98), with no container id and
    **no state id: the state id does not move**;
  - `sendDataChange` (90–97): `container_set_data(containerId, id, value)`, **no state id
    either**;
  - `createSlot` (114–126): a `RemoteSlot.Synchronized` whose hash generator is the
    synchroniser's cache (`cache::getUnchecked`, **Inferred** from the lambda's
    signature): a Guava `LoadingCache<TypedDataComponent, Integer>` of at most 256 entries
    (constructor 7–28), whose loader is `ServerPlayer$1$1` (not dumped).
  So the state id goes up once per `container_set_content` and once per
  `container_set_slot`, and nothing else moves it on the server.
- `incrementStateId()` (2443–2455): `stateId = (stateId + 1) & 32767`, returned. So it
  runs 1
  to 32767 and then 0. `setItem(slot, stateId, stack)` (1889–1899) and
  `initializeContents(stateId, items, carried)` (1901–1925) are the client's side.
- Listeners: `ServerPlayer`'s `containerListener` (`ServerPlayer$2.txt`: advancements'
  `INVENTORY_CHANGED` for a change in one of the player's own slots that is not a
  `ResultSlot`; `dataChanged` does nothing); `CrafterMenu` (3.11).
- **When `broadcastChanges` runs**: every `ServerPlayer.tick` for `containerMenu` (939–940);
  after every click (3.17); in `addSlotListener` (336–352); `slotsChanged(container)`
  (1883–1887,
  called by containers that notify their menu: in the classes dumped only by
  `CrafterSlot.setChanged`, and by the 2×2 grid's container, `TransientCraftingContainer`,
  not dumped; no block entity's container calls it, its `setChanged` marking the chunk
  and updating comparators, BlockEntity.txt 484–515); `ServerPlayer.take` (pick-up, 2716–2725); a successful
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
  Integer>`. The server's is the synchroniser's cache (3.15), whose loader
  (`ServerPlayer$1$1`) is not dumped: **Inferred** (as SteelMC's 26.3 code and 1.21.5's
  source) that it encodes the component's value with the component type's codec through
  `HashOps.CRC32C_INSTANCE` and takes the result's `asInt()`.
- **`HashOps`** (HashOps.txt, 1130 lines) is a `DynamicOps<HashCode>` over a Guava hash
  function; `CRC32C_INSTANCE` is `new HashOps(Hashing.crc32c())` (`static {}` 1052–1130).
  Every value is hashed as a tagged byte stream, and a compound's hash is made of its parts'
  hashes:

  | Value | Bytes fed to CRC32C | Lines |
  |---|---|---|
  | empty (`empty()`, a missing value) | `01` | 73–106, 1052–1059 |
  | byte | `06`, the byte | 200–212 |
  | short | `07`, `putShort` | 213–225 |
  | int | `08`, `putInt` | 226–238 |
  | long | `09`, `putLong` | 239–251 |
  | float | `0A`, `putFloat` | 252–264 |
  | double | `0B`, `putDouble` | 265–277 |
  | string | `0C`, `putInt(length in chars)`, `putUnencodedChars` | 278–292 |
  | boolean | `0D 00` (false) or `0D 01` (true), hashed once at construction | 293–303 |
  | map | `02`, then for each entry, sorted by key hash then value hash, the key's hash bytes and the value's hash bytes, then `03`; empty: `02 03` | 304–363 |
  | list | `04`, each element's hash bytes in order, `05`; empty: `04 05` | 364–385 |
  | byte array | `0E`, the bytes, `0F` | 386–407 |
  | int array | `10`, each `putInt`, `11` | 408–432 |
  | long array | `12`, each `putLong`, `13` | 433–457 |

  `createNumeric` (125–199) dispatches on the boxed type (byte, short, int, long, double,
  float), anything else as a double. A map built field by field (`mapBuilder`,
  `HashOps$MapHashBuilder.txt`) collects the pairs and hashes them as a map, sorted; a list
  builder (`HashOps$ListHashBuilder.txt`) feeds `04`, the elements, `05`. The sort's
  `HASH_COMPARATOR` is `Comparator.comparingLong` of a method reference not shown
  (**Inferred** `HashCode::padToLong`, the CRC as an unsigned 32-bit value). What the
  bytes of `putShort`, `putInt`, `putLong`, `putFloat`, `putDouble`, `putUnencodedChars`
  and `asBytes` are is Guava's, not the game's: little-endian, a char as two bytes, a
  float or double by its raw bits, and a CRC32C `HashCode` as its four bytes little-endian
  (**Inferred** from Guava's documented behaviour; to be checked against a real client).
- **What the client sends.** An empty stack is `HashedStack.EMPTY`, on the wire a single
  `false` (the optional's flag). Any other stack is its item's registry id, its count, the
  map of its *added* components (component type id → that component's hash as an `int`)
  and the set of its *removed* components (type ids only, no hash): the patch against the
  item's default components, not the whole component map. A stack whose components are all
  its item's defaults sends two empty collections.
- For Clustine's stacks without components the patch is empty: a client's hash matches a
  stack exactly when it is `EMPTY` and the stack is empty, or it is an item with the same id
  and count and both component collections empty (3.0 item 18). To match any other, the
  edge needs components in stacks, each component's codec and the hashing above.

### 3.17 `handleContainerClick` (ServerGamePacketListenerImpl.txt 5900–6052)

```
ensureRunningOnSameThread
player.resetLastActionTime()
if player.containerMenu.containerId != packet.containerId: return              // silently
if player.isSpectator() || player.isDeadOrDying(): containerMenu.sendAllDataToRemote(); return
if !containerMenu.stillValid(player): log; return                               // silently
slot = packet.slotNum
if !containerMenu.isValidSlotIndex(slot): log; return                           // 253–269
stale = packet.stateId != containerMenu.getStateId()
containerMenu.suppressRemoteUpdates()                                            // 2296–2301
containerMenu.clicked(slot, packet.buttonNum, packet.containerInput, player)
for (i, hashed) in packet.changedSlots: containerMenu.setRemoteSlotUnsafe(i, hashed)
  // 774–800: an index outside the remote slots is logged and skipped; else remoteSlots[i].receive(hashed)
containerMenu.setRemoteCarried(packet.carriedItem)                               // 802–808
containerMenu.resumeRemoteUpdates()                                              // 2303–2308
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
  1085–1094), and by `openHorseInventory` and `openNautilusInventory` (N7); nothing else in
  `ServerPlayer` calls it. A player removed from the level (`Player.remove`, n5 Player.txt
  3095–3112: logout, and any other removal) runs `inventoryMenu.removed(player)` and, if a
  container is open, `doCloseContainer()`, without the close packet.
- `doCloseContainer()` (3403–3418): `containerMenu.removed(player)`;
  `inventoryMenu.transferState(containerMenu)`; `containerMenu = inventoryMenu`.
- `AbstractContainerMenu.removed(player)` (1806–1824): only for a `ServerPlayer`: if the
  carried stack is not empty, `dropOrPlaceInInventory(player, carried)` and carried =
  `EMPTY`. `dropOrPlaceInInventory` (1826–1864): if the player is removed (for any reason
  but `CHANGED_DIMENSION`) or has disconnected, `player.drop(stack, false, SERVER_ONLY)`;
  else `inventory.placeItemBackInInventory(stack, SERVER_ONLY)`.
- `Inventory.placeItemBackInInventory(stack, prediction)` (Inventory.txt 715–723) is
  `placeItemBackInInventory(stack, true, prediction)` (724–784), a loop while the stack is
  not empty:
  ```
  slot = getSlotWithRemainingSpace(stack)  // 494–538: the selected hotbar slot, then the
                                           // off hand (40), then 0..35 in order: the first
                                           // holding the same item and components, stackable
                                           // and not full (hasRemainingSpaceForItem, 101–123)
  if slot == -1: slot = getFreeSlot()      // 124–146: the first empty slot of 0..35
  if slot == -1: player.drop(stack, false, prediction); stop
  n = stack.getMaxStackSize() - getItem(slot).count
  if add(slot, stack.split(n)) && sendPacket && player is a ServerPlayer:
    send set_player_inventory(slot, getItem(slot).copy())   // createInventoryUpdatePacket, 785–796
  ```
  So a stack put back at closing reaches the client as `set_player_inventory` packets, one
  per slot it goes into, and the empty off hand is never chosen (only a partly filled
  one).
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

- The slots are the inventory menu's (InventoryMenu.txt 51–127): 0 the 2×2 result
  (refused: `slotNum >= 1`), 1–4 the 2×2 grid, 5–8 armour (`ArmorSlot`s for `HEAD`,
  `CHEST`, `LEGS`, `FEET`, over inventory slots 39, 38, 37, 36), 9–35 the main inventory
  (inventory 9–35), 36–44 the hotbar (inventory 0–8), 45 the off hand (inventory 40, the
  anonymous `InventoryMenu$1`). The edge maps 5–45 to the inventory today
  (`Inventory::from_container`) and ignores 1–4 (3.0 item 24).
- `setByPlayer(stack)` (Slot.txt 90–97) is `setByPlayer(stack, getItem())`; on an armour
  slot (ArmorSlot.txt) that runs `owner.onEquipItem(slot, old, stack)` (the equip sound and
  equipment change, n5 LivingEntity) before the set. `ArmorSlot` also has
  `getMaxStackSize` 1, `mayPlace` = `owner.isEquippableInSlot(stack, slot)`, and
  `mayPickup` false for a non-empty slot whose item has the `prevent_armor_change`
  enchantment effect (the curse of binding) unless the player is creative. The creative
  packet asks none of these: it sets the slot.
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

- `inventoryMenu` (container id 0, `InventoryMenu`, InventoryMenu.txt) is always there and
  is `containerMenu` when nothing else is open. Its 46 slots are as 3.19's list; slot 0 is
  a `ResultSlot` (ResultSlot.txt: `mayPlace` false; taking from it crafts) over a
  `ResultContainer`, and the grid is a `CraftingContainer` of 2×2. `stillValid` is always
  true (189–193). `removed` (169–188) clears the result and, on the server, puts the
  grid's items back with `clearContainer` (each `dropOrPlaceInInventory`, 3.18); it runs
  only when the player is removed. `quickMoveStack` (194–390), in this order of tests: from
  the result (0) to 9–44 from the end (`moveItemStackTo(stack, 9, 45, true)`, then
  `onQuickCraft`); from the grid (1–4) or armour (5–8) to 9–44; an item whose equipment
  slot is a humanoid armour slot to that armour slot (menu slot `8 − slot.getIndex()`,
  so 5 for `HEAD`) if it is empty; an off-hand item (a shield) to 45 if empty; from the
  main inventory (9–35) to the hotbar (36–44); from the hotbar (36–44) to the main
  inventory (9–35); anything else (the off hand) to 9–44. Then, as the others, `EMPTY` if
  nothing moved, else `onTake`; from the result, what is left in it is dropped
  (`player.drop(stack, false, PREDICTED)`, 374–388). A survival
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
   default is true (`registerBoolean("tnt_explodes", MISC, true)`, GameRules.txt
   777–787). The PrimedTnt is made at the
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
    every projectile (N9), spawn eggs (N7), bone meal (its growth is each block's: 4.12), the
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
  `min(stack.count, max − slot.count)` (with `max = getMaxStackSize(stack)`, which is
  `Math.min(getMaxStackSize(), stack.getMaxStackSize())` with the container's default
  99, Container.txt 17–29, not overridden by the dispenser): into an empty slot by
  `setItem(i, stack.split(k))`, into a matching one by
  shrinking `stack` and growing the slot; stop once `stack` is empty. Returns what is
  left.
- **Loot tables**: a dispenser made by a structure (`chests/jungle_temple_dispenser`:
  1–2 rolls of 2–7 arrows) unpacks its table the first time a slot is chosen
  (`getRandomSlot` calls `unpackLootTable(null)` first) or the menu is opened.
  `RandomizableContainer.unpackLootTable(player)` (RandomizableContainer.txt 102–167): if
  a table is set and the level has a server, (a `ServerPlayer` gets the `generate_loot`
  trigger), the table is cleared **before** it is filled (`setLootTable(null)`), then
  `LootTable.fill(container, params, getLootTableSeed())` with the `CHEST` parameter set:
  `ORIGIN` the block's centre and, with a player, `withLuck(player.getLuck())` and
  `THIS_ENTITY`. It draws nothing from the level itself; the loot's draws come from the
  seed (`LootTable.fill` is not read: what a seed of 0 means, section 12). The seed was
  drawn when the structure placed the table (`setBlockEntityLootTable`, lines 21–47:
  one `nextLong` of the generator's random). `tryLoadLootTable` (48–73) reads
  `LootTable` and `LootTableSeed` (0 if absent) and answers whether a table was there;
  `trySaveLootTable` (74–101) writes `LootTable` and `LootTableSeed` only when not 0.

### 4.2 The tick: `dispenseFrom(level, state, pos)` (DispenserBlock.txt 91–156)

```
be = level.getBlockEntity(pos, DISPENSER) or return (a warning)
source = BlockSource(level, pos, state, be)      // state: the tick's state, as passed
slot = be.getRandomSlot(level.getRandom())        // 4.0 item 2: unpack, then one nextInt per non-empty slot
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
item = new ItemEntity(level, x, y, z, stack)         // its own random: uuid (2 nextLong), bob, yaw, motion (n4 8.4)
r = level.getRandom()
s = r.nextDouble() * 0.1 + 0.2                        // level draw 1
item.setDeltaMovement(
    r.triangle(dir.stepX * s, 0.0172275 * accuracy),  // level draws 2-3 (two nextDouble)
    r.triangle(0.2,           0.0172275 * accuracy),  // level draws 4-5
    r.triangle(dir.stepZ * s, 0.0172275 * accuracy))  // level draws 6-7
level.addFreshEntity(item)
```

The constructor's motion is replaced; its draws are made all the same (Clustine's
`spawn_item_as` with a `motion` does this, `region/drops.rs` 136–175). No
`setDefaultPickUpDelay`: the item keeps the constructor's delay (0, as
`Containers.dropItemStack`'s, `drops.rs`). `0.0172275 * 6` is a `double` product computed
at run time (`ldc2_w 0.0172275`, `i2d`, `dmul`). `triangle(a, b)` is
`a + b · (nextDouble() − nextDouble())` with the two draws in that order
(RandomSource.txt 77–88): each `triangle` is two `nextDouble`, so `spawnItem` makes seven
level draws. Then
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
1000 + 2000), then the outer `dispense` plays 1000 + 2000 again. Clustine's
`placement/bucket.rs` has the player's version to share.

`BucketItem.emptyContents(user = null, level, pos, hit = null)` (BucketItem.txt
212–425), as a dispenser calls it:

```
if content is not a FlowingFluid: return false           // the empty bucket
state = level.getBlockState(pos); block = state.getBlock()
replaceable = state.canBeReplaced(content)
sneaking = user != null && user.isShiftKeyDown()          // false here
canPlace = replaceable || (block is LiquidBlockContainer
                           && block.canPlaceLiquid(user, level, pos, state, content))
if !(state.isAir() || (canPlace && (!sneaking || hit == null))):
    return hit != null && emptyContents(user, level, hit.pos.relative(hit.dir), null)
                                                          // hit is null: false
if level.environmentAttributes().getValue(WATER_EVAPORATES, pos) && content is #water:
    playSound(user, pos, FIRE_EXTINGUISH, BLOCKS, 0.5f,
              2.6f + (r.nextFloat() - r.nextFloat()) * 0.8f)   // two level draws, float
    sendParticles(LARGE_SMOKE, (double) x, (double) y, (double) z,   // the block's corner
                  8, 1.0, 1.0, 1.0, 0.0, ALTERNATIVE)
    return true                                           // nothing placed
if block is LiquidBlockContainer && content == WATER:     // waterlogging
    block.placeLiquid(level, pos, state, WATER.getSource(false)); playEmptySound; true
if !level.isClientSide() && replaceable && !state.liquid():
    level.destroyBlock(pos, true)                         // drops the replaced block
if level.setBlock(pos, content.defaultFluidState().createLegacyBlock(), 11)
   || state.getFluidState().isSource():
    playEmptySound(user, level, pos); return true
return false
```

`playEmptySound` (427–452): `bucket.empty_lava` for `#lava`, else `bucket.empty`, at the
block, `BLOCKS`, volume and pitch 1, then `gameEvent(user, FLUID_PLACE, pos)`. A water
bucket into a source of water still answers true (the set fails, the old state is a
source): the dispenser keeps an empty bucket and the water stays.
`BlockState.canBeReplaced(Fluid)` is the block's `canBeReplaced(state, fluid)`
(BlockBehaviour$BlockStateBase.txt 1115–1123), by default `state.canBeReplaced() ||
!state.isSolid()` (BlockBehaviour.txt 352–364); `LiquidBlock`'s own override, if any, is
not dumped. `checkExtraContent` is empty in
`BucketItem` (208–211); the mob buckets override it (N7).

**The empty bucket** (`$4`, 9–73): `target = pos + facing`; `b =
level.getBlockState(target)`;
if its block is a `BucketPickup`: `filled = pickupBlock(null, level, target, b)`; empty
result → the default `execute` (drop the bucket, one item); else
`gameEvent(null, FLUID_PICKUP, target)` and `consumeWithRemainder(source, stack, new
ItemStack(filled.getItem()))`. Not a `BucketPickup` → the default `execute` (drops the
bucket). `BucketPickup` is an interface of two abstract methods (`pickupBlock(user,
level, pos, state)` and `getPickupSound()`, BucketPickup.txt 3–5); its implementations
(the liquid block's source check, waterlogged blocks drained, powder snow) are not
dumped (section 12). The sound of filling is the block's `getPickupSound`, played by
`BucketItem.use` for a player but **not here**: `$4` has no `playSound`. Only one
1000 + 2000 here (`$4` calls `super.execute`, not `dispense`).

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
blocks included. `toYRot` is `(float)((data2d & 3) * 90)` (Direction.txt 898–908), and
`up` and `down` have `data2d = −1` (the enum's `<clinit>`, 1269–; south 0, west 1,
north 2, east 3), so a stand or boat from a vertical dispenser gets **270°**, the yaw of
`east`. Sound and smoke: the default's 1000 + 2000.

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
elif level.getBlockState(front).isAir() and level.getFluidState(front.below()).is(#water): lift = 0.0
else: return inner.dispense(source, stack)              // drop one: 1000 + 2000 twice in all
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
y = Math.floor(c.y) + stepY                            // the dispenser's own y plus the step
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
slot = e.getEquipmentSlotForItem(stack)               // the equippable's slot if e.canUseSlot(it), else MAINHAND
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
if !level.gameRules.get(TNT_EXPLODES): setSuccess(false); return stack     // 1001, item kept
front = pos + facing
if SulfurCubeBlockDispenseItemBehavior.dispenseBlock(level, front, stack): return stack // a cube took it (success stays as last set!)
t = new PrimedTnt(level, front.x + 0.5, front.y, front.z + 0.5, null)     // PrimedTnt.txt 43-90: one level nextDouble
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
   a nether portal) follows as for any fire: `FireBlock.onPlace` (FireBlock.txt
   818–836) runs `BaseFireBlock.onPlace` (not dumped: the portal) and then
   `scheduleTick(pos, fire, 30 + level.random.nextInt(10))` (`getFireTickDelay`,
   837–845; **one level draw**), as Clustine's `random_ticks/fire.rs` has it.
3. else if `CampfireBlock.canLight(b) || CandleBlock.canLight(b) ||
   CandleCakeBlock.canLight(b)`: `setBlockAndUpdate(front, b.setValue(LIT, true))` and
   `gameEvent(null, BLOCK_CHANGE, front)`.
4. else if `b.getBlock() instanceof TntBlock`: `TntBlock.prime(level, front, null,
   stack)` (TntBlock.txt 140–221: false when `tnt_explodes` is off; else a PrimedTnt at
   `(x + 0.5, y, z + 0.5)`, one level draw, `TNT_PRIMED` at the entity, `PRIME_FUSE` game
   event at the block) and, if true, `removeBlock(front, false)`; if false
   `setSuccess(false)`.
5. else nothing (`success` stays true: **the click is 1000 though nothing happened**).

Then if `success`: `stack.hurtAndBreak(1, level, null, onBreak)` (ItemStack.txt
744–762): `processDurabilityChange` (764–786) answers 0 for an item that cannot be
damaged and for a player with infinite materials (the player is null here, so never),
else, for an amount above 0, `EnchantmentHelper.processDurabilityChange(level, stack,
amount)` (not dumped: Unbreaking's chance, a draw only with that enchantment,
**Inferred**); a non-zero result sets `damage + n` and, once `damage ≥ max_damage`,
shrinks the stack by one (a copy goes to `onBreak`, here nothing) (`applyDamage`,
788–). So a plain flint and steel takes one point per success and draws nothing.
Sound by success (1000/1001); smoke 2000. The flint and steel stays in the slot until
broken. `BaseFireBlock.canBePlacedAt`/`getState` and the `canLight`s are not dumped
(section 12); Clustine's fire code knows the fire's state.

### 4.12 Bone meal (`$5`, 51 lines)

`setSuccess(true)`; `front = pos + facing`; if `BoneMealItem.growCrop(stack, level,
front)` or `BoneMealItem.growWaterPlant(stack, level, front, null)`: `levelEvent(1505,
front, 15)` (`PARTICLES_AND_SOUND_PLANT_GROWTH`, data 15); else `setSuccess(false)`.
`BoneMealItem` (BoneMealItem.txt):

- `growCrop(stack, level, pos)` (92–144): the block must be a `BonemealableBlock` with
  `isValidBonemealTarget(level, pos, state)`; on the server, `if
  isBonemealSuccess(level, level.random, pos, state)` then `performBonemeal(level,
  level.random, pos, state)`; then **`stack.shrink(1)` whether or not it grew**, and
  true. (The success's and the growth's draws are each block's; the
  `BonemealableBlock`s are not dumped.)
- `growWaterPlant(stack, level, pos, null)` (145–357): only at a `water` block whose
  fluid is full (a source); then 128 attempts `i = 0..127`, each a walk of `i / 16`
  steps (0 to 7) from `pos` by `offset(nextInt(3) − 1, (nextInt(3) − 1) · nextInt(3) / 2,
  nextInt(3) − 1)` (**four level draws a step**, in that order, `int` division); an
  attempt whose walk meets a full collision cube is abandoned; at the end of a walk: in a
  biome of `#produces_corals_from_bonemeal`, with `i ≠ 0` or the dispenser's null face,
  one `nextInt(4) == 0`
  chooses a random block of `#underwater_bonemeals` (`getRandomElementOf`, one more
  draw); a wall coral is turned by `Plane.HORIZONTAL.getRandomDirection` until it
  survives (up to four tries); then, if the end block is full water, it is set
  (`setBlockAndUpdate`), else if it is seagrass that can be grown, one `nextInt(10) == 0`
  grows it (`performBonemeal`). The stack shrinks once and the answer is true, grown or
  not. Hundreds of level draws per use.
- The particles and sound of 1505 are the client's (`addGrowthParticles`, 358–419).

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
item shrunk).

`DirectionalPlaceContext(level, pos, direction, stack, face)` (DirectionalPlaceContext.txt
5–262) is a `BlockPlaceContext` with no player, the main hand, and a hit at
`Vec3.atBottomCenterOf(pos)` on face `face` of `pos` itself (not inside). So
`getClickedPos` is `pos` (26–32), `canPlace` is `level.getBlockState(pos).canBeReplaced(
this)` (33–44), `replacingClickedOnBlock` is `canPlace` (45–50), `getClickedFace` is
`face` (= `dir`), `getNearestLookingDirection` is **`DOWN`** (51–55, not `facing`: the
earlier reading's guess was wrong), `getNearestLookingDirections` an order from the
`$1` switch on `direction` (56–234), `getHorizontalDirection` is `direction`, or `NORTH`
when vertical (235–247), `isSecondaryUseActive` false, `getRotation` `data2d · 90` (no
`& 3`: −90 for a vertical dispenser; 253–262). The shulker box takes its facing from
the clicked face, so it faces `facing`, or up when the block under the front is not
empty.

`BlockItem.place(ctx)` (BlockItem.txt 55–184), in order: the block not enabled by the
level's features → `FAIL`; `!ctx.canPlace()` → `FAIL`; `updatePlacementContext` (null →
`FAIL`); `getPlacementState` (212–230: the block's `getStateForPlacement`, kept only if
`canPlace(ctx, state)`, 260–288: `canSurvive` where `mustSurvive()` (true), and
`isUnobstructed(state, pos, CollisionContext.placementContext(player))`: **a box is not
placed into an entity's box**) (null → `FAIL`); `placeBlock` (294–304: `setBlock(pos,
state, 11)`; false → `FAIL`); then, if the block at `pos` is still the placed block:
`updateBlockStateFromTag` (the `block_state` component), `updateCustomBlockEntityTag`
(the `block_entity_data` component), `updateBlockEntityComponents` (197–211:
`applyComponentsFromItemStack`, so the box's `container` and name, then `setChanged`),
`setPlacedBy`, the advancement for a player; then the place sound
`playSound(player, pos, soundType.getPlaceSound(), BLOCKS, (volume + 1) / 2, pitch ·
0.8f)` (heard by everyone, the player being null), `gameEvent(BLOCK_PLACE, pos,
Context.of(player, state))`, `stack.consume(1, player)`, `SUCCESS`. Clustine's block
placing (`placement`) is the player's; the dispenser's needs this context without a
player. Without components in Clustine's stacks (section 0) a dispensed box is placed
empty.

### 4.16 Shears, brush, chest on a horse, spawn eggs (mostly later steps)

- **Shears** (`ShearsDispenseItemBehavior`, 182 lines): `front`; `tryShearBeehive`
  (53–107: a beehive with honey 5: `BEEHIVE_SHEAR` sound at `front`,
  `BeehiveBlock.dropHoneycomb`, `releaseBeesAndResetHoneyLevel`, `SHEAR` game event)
  or `tryShearEntity` (109–162): for each entity (any class, `NO_SPECTATORS`) in
  `AABB(front)`: **`shearOffAllLeashConnections(null)` true → done** (N5's leashes:
  a leashed boat in front is let go, the lead dropped); else a live `Shearable` ready
  for shearing → `shear(level, BLOCKS, stack)` and `SHEAR` (N7). Success → one point of
  durability. **The leash part is N6's** (N5's `Leashable` is built).
  `Entity.shearOffAllLeashConnections(player)` (Entity.txt 6681–6713):
  `dropAllLeashConnections(player)` (6714–): the entities leashed to this one
  (`Leashable.leashableLeashedTo(this)`), and this one itself if it is a `Leashable`
  that is leashed, each `dropLeash()` (the lead dropped as an item, N5 7); if any was,
  `gameEvent(SHEAR, player)` and true. Then, on the server, `playSound(null,
  blockPosition(), shears.snip, the player's sound source or else the entity's)`.
  So shears free a knot's boats and a boat that holds others, not only a leashed boat.
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
`levelEvent(override or 1002, pos, 0)`. `ProjectileItem$DispenseConfig` (DispenseConfig
.txt) holds `positionFunction`, `uncertainty` (float), `power` (float) and
`overrideDispenseEvent` (`Optional<Integer>`); `DEFAULT` is `builder().build()`, whose
defaults are in `DispenseConfig$Builder` (not dumped), and each item's own config is in
its item class (not dumped). `ProjectileItem.shoot` by default is
`projectile.shoot(x, y, z, power, uncertainty)` (ProjectileItem.txt 10–). The items:
arrow, tipped_arrow, spectral_arrow,
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
chest boats and rafts; the predicate is `entity instanceof Container &&
entity.isAlive()`, EntitySelector.txt 206–218, `lambda$static$2`, the fourth predicate
made in `<clinit>`, 257: the lambda's order, not a `javap -v`) whose box meets the box of
half-size 0.5
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
- **Entities added**: `ServerLevel.addFreshEntity` → `addEntity` (ServerLevel.txt
  2015–2120) only adds the entity (refused if removed) to the entity manager; it posts
  no game event, so the `ENTITY_PLACE` posts above are the behaviours' own.
- **Items**: `region/drops.rs` `spawn_item_as` (149) with an explicit motion and a delay
  of 0 is `spawnItem` once the seven level draws are drawn from the block's stream (n4
  8.5's proposal); `region/items.rs` has the item's tick and pick-up.
- **Boats and carts**: `region/boats.rs` `place_boat` (333) and `region/minecarts.rs`
  `place_minecart` (274) make the entities from a player's click; the dispenser's paths
  compute other positions and skip the boat's collision check, but share the entity's
  making (`kind_of_item`, `boat_of_item`, the entity data).
- **Buckets**: `placement/bucket.rs` has the player's bucket use (empty and filled); the
  dispenser's needs `emptyContents` and `pickupBlock` at a block without a ray.
- **Fire**: `region/random_ticks/fire.rs` has fire's ticks and `onPlace`'s parts; the
  dispenser needs `BaseFireBlock.canBePlacedAt` and `getState` (not dumped).
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
   player. `RandomizableContainer.unpackLootTable(null)` (RandomizableContainer.txt
   102–167) clears the table first (`setLootTable(null)`), then
   `LootTable.fill(container, params, LootTableSeed)` with `ORIGIN` the block's centre
   and the `CHEST` parameter set, without luck or `THIS_ENTITY` (those come only with a
   player); `fill` itself is not dumped (section 12).
8. **A hopper block entity sends nothing to clients.** `HopperBlockEntity` overrides
   neither `getUpdatePacket` nor `getUpdateTag`; its contents reach a client only
   through the menu (section 3). Its block state changes only for `ENABLED` and is sent
   as any block change.
9. **`ENABLED` is set with flag 2 only** (clients, no neighbour or shape updates) on
   every `neighborChanged` and on `onPlace` when the block was not already a hopper:
   `ENABLED = !hasNeighborSignal(pos)` (5.1).
10. **A hopper cart has no cooldown, never pushes, and takes once a tick.** It sucks
    after its move each tick (and, under the new behaviour only, after each step along
    the track), at most once per tick (`consumedItemThisFrame`), only while `enabled`,
    which an activator rail sets to `!powered` every tick the cart is on one. It is
    emptied only by a hopper below or beside it pulling or pushing through the entity
    search. Its `Enabled` is saved (default true) and not synched; Clustine's
    `EntityData::Minecart` has no field for it yet (5.9).
11. **The composter is a hopper's source and target** through `WorldlyContainerHolder`:
    a hopper above it (facing down) composts one compostable item per move, a hopper
    below takes the bone meal when it is ready, and a hopper beside it does nothing. A
    composter above a hopper always counts as its source, so that hopper never sucks
    item entities. A hopper that cannot hold the bone meal still empties the composter,
    and the bone meal is lost (5.6).
12. **Stack sizes**: a container's limit is `min(99, item's max stack size)`
    (`Container.getMaxStackSize`, Container.txt 17–29); `setItem` truncates a stack to
    it (`ItemStack.limitSize`), and the hopper's empty-slot path hands the whole
    incoming stack to `setItem`, so an over-size stack from an item entity loses its
    excess (only from commands). Clustine's `ItemStack` has an item and a count and no
    components: `isSameItemSameComponents` reduces to the item until components exist.

### 5.1 The block: `HopperBlock`

- **Properties**: `FACING` = `BlockStateProperties.FACING_HOPPER` (down, north, south,
  west, east: no up), `ENABLED` (static, 357–363). Default `FACING = DOWN, ENABLED =
  true` (constructor, 11–59).
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
  y1)` is a column `w` wide centred in the block from `y0` to `y1`: `column(w, w, y0,
  y1)` = `box(8 − w/2, y0, 8 − w/2, 8 + w/2, y1, 8 + w/2)`, each coordinate then divided
  by 16 (Block.txt 161–188, 221–255). Base = `column(16, 10, 16) ∪ column(8, 4, 10)`,
  less the bowl `column(12, 11, 16)` (`ONLY_FIRST`); the spout `boxZ(4, 4, 8, 0, 8)` (4
  wide, y 4–8, z 0–8: `boxZ(w, y0, y1, z0, z1)` = `box(8 − w/2, y0, z0, 8 + w/2, y1,
  z1)`, Block.txt 284–310) rotated to `FACING` about (8, 6, 8)/16 (`rotateAll`), cut to
  the block (`AND Shapes.block()`) and added. `getShape` ignores `ENABLED`
  (`getShapeForEachState(…, ENABLED)` lists it as ignored). Interaction shape
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
  `TransferCooldown` (int). Name: `container.hopper` (static, 1141–1149), or a custom
  name (`BaseContainerBlockEntity`, section 3).
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
  (by `WorldlyContainerHolder`, 5.6), furnaces and the brewing stand (not dumped).
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
   ticker's or the walk's state). The ticker is called by
   `LevelChunk$BoundTickingBlockEntity.tick` (LevelChunk$BoundTickingBlockEntity.txt
   29–120) only when the block entity is not removed, has a level and its chunk
   `isTicking(pos)`, with the chunk's **current** state at `pos`, and only if
   `type.isValid(state)` (the block entity type admits that block); otherwise it logs
   "invalid for ticking" once and skips. So a hopper whose block was replaced by
   another while its block entity remains is not ticked.
2. `moved = false`. If the hopper is not `isEmpty()`
   (`RandomizableContainerBlockEntity.isEmpty`, 45–52: `unpackLootTable(null)`, then
   every slot empty, BaseContainerBlockEntity.txt 135–156):
   `moved = ejectItems(level, pos, hopper)`.
3. If not `inventoryFull()` (223–248: some slot empty or below its item's max stack
   size): `moved |= sucker.getAsBoolean()` (the `|` evaluates the sucker whatever `moved`
   is).
4. If `moved`: `setCooldown(8)`; `setChanged(level, pos, state)`; true. Else false.

### 5.5 Sucking: `suckInItems(level, hopper)` (415–507)

`hopper` is a block hopper or a hopper cart (both `Hopper`). With `(X, Y, Z) =
(getLevelX(), getLevelY(), getLevelZ())` (block: the block's centre, 999–1027; cart: its
position with `y + 0.5`, MinecartHopper.txt 69–87):

1. `above = BlockPos.containing(X, Y + 1, Z)`, `aboveState` its state.
2. `source = getSourceContainer(level, hopper, above, aboveState)` (804–818) =
   `getContainerAt(level, above, aboveState, X, Y + 1, Z)` (5.6).
3. **With a source**: `face = DOWN`; for each `slot` of `getSlots(source, DOWN)` in
   order: if `tryTakeInItemFromSlot(hopper, source, slot, DOWN)`: return true. Return
   false (item entities are not looked at).
4. **Without**: `blocked = hopper.isGridAligned() &&
   aboveState.isCollisionShapeFullBlock( level, above) &&
   !aboveState.is(#does_not_block_hoppers)` (block hoppers are grid aligned, 1029–1032;
   carts are not). If not `blocked`: for each item entity of `getItemsAtAndAbove(level,
   hopper)` in order: if `addItem(hopper, entity)`: return true. Return false.

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
(its own `y` + 0.5 − 0.5). Order: `getEntitiesOfClass`'s, which is the sections'
order of `EntitySectionStorage.forEachAccessibleNonEmptySection`
(EntitySectionStorage.txt 37–148), then each section's own order (5.6, "The order of entity queries").

**`addItem(container, itemEntity)`** (557–586): `copy = entity.getItem().copy()`;
`rest = addItem(null, container, copy, null)`; if `rest` empty: `entity.setItem(EMPTY)`,
`entity.discard()`, true; else `entity.setItem(rest)`, false. `ItemEntity.setItem`
(ItemEntity.txt 998–1006) only sets the synched `DATA_ITEM`, so a partial take is an
entity-data change sent to trackers (a new stack object each time, so marked dirty:
**Inferred**, `SynchedEntityData.set` not dumped); `discard` removes the entity at once.
Nothing here or in `ItemEntity` looks at the item's `pickupDelay` (ItemEntity.txt
173–200 lowers it in the item's own tick; `playerTouch`, 893–948, and the merging,
535–564 and 657–683, read it): a
hopper takes an item just thrown or dropped, whatever its delay, and also an item that
can never be picked up (`pickupDelay` 32767).

### 5.6 Finding containers: `getContainerAt` (845–937), `getEntityContainer` (939–981)

- `getContainerAt(level, pos)` (845–868) = the six-argument form with the block's centre
  `(x + 0.5, y + 0.5, z + 0.5)` (ints widened, then added).
- `getContainerAt(level, pos, state, x, y, z)` (870–886): `getBlockContainer(level, pos,
  state)`, else `getEntityContainer(level, x, y, z)`.
- **`getBlockContainer`** (888–937): a block that is a `WorldlyContainerHolder` (in 26.3
  only `ComposterBlock` implements it, ComposterBlock.txt 1) gives `getContainer(state,
  level, pos)` (the composter's, below); else, if `state.hasBlockEntity()` and the block
  entity is a `Container`: if it is a `ChestBlockEntity` and the block a `ChestBlock`,
  `ChestBlock.getContainer(block, state, level, pos, true)` (the double chest's compound,
  **ignoring what blocks it**: a cat or a block above does not stop a hopper; section 3),
  else the block entity. Else null. Block entities that are `Container`s in 26.3
  (**Inferred** for the classes not dumped, section 12): chest and trapped chest (and
  copper chests, which use `CHEST`), barrel, dispenser, dropper, hopper, shulker boxes,
  crafter (all read); furnace, blast furnace, smoker, brewing stand, chiseled bookshelf,
  decorated pot, jukebox, shelf (not read). The lectern and the ender chest are not.
- **`getEntityContainer(level, x, y, z)`** (939–981): `list = level.getEntities(null,
  AABB(x − 0.5, y − 0.5, z − 0.5, x + 0.5, y + 0.5, z + 0.5),
  CONTAINER_ENTITY_SELECTOR)`; empty: null; else
  `list.get(level.getRandom().nextInt(list.size()))`. `CONTAINER_ENTITY_SELECTOR` is `e
  instanceof Container && e.isAlive()` (EntitySelector.txt `lambda$static$2`, 206–218;
  **Inferred** which lambda belongs to which field, by the order of the static
  initialiser, 245–272). Among N5's entities: `MinecartChest`, `MinecartHopper`,
  `ChestBoat`, `ChestRaft`. The box is tested by intersection (`AABB.intersects` is
  strict, AABB.txt 923–970 in the N5 dump; **Inferred** that the section's test is
  `intersects`, `EntitySection` not dumped).
- **The composter** (`ComposterBlock.getContainer`, ComposterBlock.txt 512–548) is a
  fresh container object made for each lookup, by `LEVEL`:
  - `LEVEL` 8 (ready): an `OutputContainer` holding one bone meal
    (ComposterBlock$OutputContainer.txt): slot 0 on face `DOWN` only, max stack 1,
    nothing placed through any face, bone meal taken through `DOWN` while not yet
    `changed`. Its `setChanged` calls `ComposterBlock.empty(null, state, level, pos)`
    (351–374: `LEVEL` 0 by `setBlockAndUpdate`, game event `block_change` with the
    composter's state) and sets `changed`.
  - `LEVEL` 0 to 6: an `InputContainer` (ComposterBlock$InputContainer.txt), one empty
    slot, max stack 1, slot 0 on face `UP` only, an item placed through `UP` when it has
    the `compostable` component and the container is not yet `changed`, nothing taken.
    Its `setChanged`, when slot 0 holds a compostable item and the level is a server
    level: `changed = true`; `new = addLayer(null, state, level, pos, compostable)`;
    `levelEvent(1500, pos, new != state ? 1 : 0)` (the composter's fill: sound and
    particles, **Client**); slot 0 emptied (`removeItemNoUpdate`).
  - `LEVEL` 7: an `EmptyContainer` (size 0, no slots on any face, nothing in or out).
  - `addLayer` (375–452): `layers = compostable.layers().get(ctx, 0)` with a loot
    context of the block state, the origin and the (null) entity (`BLOCK_INTERACT`);
    the item's layers may be a chance (a draw, of the loot context's random, which with
    no seed is the level's: **Inferred**, `LootContext$Builder` not dumped). If `layers
    > 0`: `LEVEL = clamp(old + layers, 0, 7)` by `setBlockAndUpdate`, game event
    `block_change`, and at 7 `scheduleTick(pos, composter, 20)`; the tick (453–479)
    turns 7 into 8 and plays `composter_ready` (1, 1). Else the state is unchanged.
  - **A hopper above a composter (facing down) feeds it**: the face is `UP`; the hopper
    hands one item to `setItem(0, …)` (`SimpleContainer.setItem` calls `setChanged`,
    SimpleContainer.txt 223–238), which composts it at once; the move counts as
    successful whether or not a layer was added (the item is gone either way). A
    hopper beside a composter finds no slot on its face (`isFullContainer` of an empty
    slot list is true) and does nothing.
  - **A hopper below a composter pulls the bone meal** at `LEVEL` 8: `removeItem(0, 1)`
    (`SimpleContainer.removeItem` calls `setChanged` on a non-empty removal, 70–84)
    empties the composter at once; the hopper's success calls `setChanged` again, which
    sets `LEVEL` 0 once more (no change) and posts a second `block_change`. If the bone
    meal does not fit (every hopper slot taken by other items, some not full), the hopper
    puts the stack back with `setItem`, which runs `empty` a second time (a second
    `block_change`): **the composter is emptied and the bone meal is lost**. A composter
    above a hopper is always a source (even at `LEVEL` 0 to 7, with no slot on `DOWN`),
    so a hopper under a composter never sucks item entities.
- **The order of entity queries** (`Level.getEntities`, Level.txt 1261–1340;
  `EntitySectionStorage.forEachAccessibleNonEmptySection`, EntitySectionStorage.txt
  37–148): the sections visited are those from `floor(minX − 2) >> 4` to `floor(maxX +
  2) >> 4` in x, `floor(minY − 4) >> 4` to `floor(maxY) >> 4` in y, and `floor(minZ −
  2) >> 4` to `floor(maxZ + 2) >> 4` in z (an entity is filed by its position's
  section, so a box reaching up to 2 blocks across in x and z, and 4 below, is caught);
  x ascending as an `int`; for each x, the existing sections in the order of the sorted
  set of section keys (`SectionPos.asLong`: **Inferred** the usual packing, x in the
  top 22 bits, z in the next 22, y in the low 20, each two's complement in its field,
  so within one x the order is by z, then y, each with 0 and positive first, then
  negative), skipping sections that are empty or not accessible
  (`Visibility.isAccessible`); within a section, the section's own order (**Inferred**
  insertion order, `EntitySection`/`ClassInstanceMultiMap` not dumped). Ender dragon
  parts are added after, in `getEntities(Entity, AABB, Predicate)` only.

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
the ticker's. The walk runs in the item's tick (`applyEffectsFromBlocks`, after its
move), so the item enters the hopper in the entities' phase; Clustine's phases are block
events, entities, block entities (ADR-0026, "Found while building"), the same order.

### 5.9 `MinecartHopper` (MinecartHopper.txt)

- `extends AbstractMinecartContainer implements Hopper`; 5 slots (34–37); display block
  a hopper, offset 1 (23–32); drops `HOPPER_MINECART` (184–187); menu `HopperMenu`
  (222–230, section 3).
- `enabled` (default true, 9–21); saved as `Enabled` (`putBoolean`, 197–207), read with
  default true (209–220). Not in its synched data.
- **`activateMinecart(level, x, y, z, powered)`** (39–54): `enabled = !powered` if it
  differs. Called every tick the cart is on an activator rail (N5 brief 3.7; the repo's
  `activate_minecart` in `minecarts.rs` leaves it out today).
- **`tick()`** (94–103): `consumedItemThisFrame = false`; `super.tick()` (the cart's
  whole tick, its move included); `tryConsumeItems()`.
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
- Needed: a typed block entity `Hopper { items: [ItemStack; 5], cooldown: i32, ticked:
  u64 }` (`ticked` in memory only, 0 after a load, as the game's), durable as the game
  saves it (`Items`, `TransferCooldown`, an optional `LootTable`), a ticker in the list
  of `block_entities.rs`, and container access for the other block entities (section 3's
  typed containers): chest (with its double), barrel, dispenser, dropper, shulker box,
  crafter, the composter's three made-up containers by its `LEVEL` (no block entity:
  built from the state at each lookup, its moves block changes with game events and the
  fill's level event 1500), and a fallback that leaves an unported container (furnace…)
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
3. **Only the stone and polished blackstone plates are `MOBS`** (`BlockSetType`, static
   initialiser, BlockSetType.txt 260–500): every wooden plate (the eleven woods of the
   one-argument constructor, BlockSetType.txt 71–90, and cherry, crimson, warped and
   bamboo with their own sounds) and both weighted plates are `EVERYTHING`; the poplar
   plate (`poplar_pressure_plate`) is among the wooden ones. Clustine's plates treat all
   alike, as only players pressed them so far (6.2).
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
   with flags 260 (`UPDATE_NONE`, that is `UPDATE_INVISIBLE` and
   `UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS`, Block$UpdateFlags.txt 11–37) and emits `GameEvent.SHEAR`, then breaks it
   as usual: the hooks recalculate with a disarmed wire and do not click on. In creative
   too: `ServerPlayerGameMode.destroyBlock` (N5 dump, ServerPlayerGameMode.txt 788–)
   calls `playerWillDestroy` for every break that `ItemStack.canDestroyBlock` allows,
   before `removeBlock`.
9. **Game events.** A plate's change emits `BLOCK_ACTIVATE` or `BLOCK_DEACTIVATE` with
   the entity that pressed it (null from its scheduled tick); the hooks' `emitState`
   emits `BLOCK_ACTIVATE`, `BLOCK_DEACTIVATE`, `BLOCK_ATTACH`, `BLOCK_DETACH` (null
   entity); tripwire shorn emits `SHEAR` with the player. They matter only to sculk
   sensors (section 8).
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
- **The block set of each plate** (`Blocks`, the plates' constructors in
  `lambda$static$131` to `$143`, `$250`, `$251`, `$336`, `$337`, `$356`; `BlockSetType`
  static initialiser, BlockSetType.txt 260–500; sounds are the set's
  `pressurePlateClickOn`/`Off`, played at volume 1, pitch 1):

  | Plate | Set | Sensitivity | Click on / off |
  |---|---|---|---|
  | `stone_pressure_plate` | `stone` | `MOBS` | `block.stone_pressure_plate.click_on` / `_off` |
  | `polished_blackstone_pressure_plate` | `polished_blackstone` | `MOBS` | the stone plate's |
  | `oak`, `spruce`, `birch`, `jungle`, `acacia`, `dark_oak`, `pale_oak`, `mangrove`, `poplar` `_pressure_plate` | the wood's (one-argument constructor) | `EVERYTHING` | `block.wooden_pressure_plate.click_on` / `_off` |
  | `cherry_pressure_plate` | `cherry` | `EVERYTHING` | `block.cherry_wood_pressure_plate.click_on` / `_off` |
  | `crimson_pressure_plate`, `warped_pressure_plate` | `crimson`, `warped` | `EVERYTHING` | `block.nether_wood_pressure_plate.click_on` / `_off` |
  | `bamboo_pressure_plate` | `bamboo` | `EVERYTHING` | `block.bamboo_wood_pressure_plate.click_on` / `_off` |
  | `light_weighted_pressure_plate` | `gold` | (weighted) | `block.metal_pressure_plate.click_on` / `_off` |
  | `heavy_weighted_pressure_plate` | `iron` | (weighted) | `block.metal_pressure_plate.click_on` / `_off` |

  (The `copper` set, also `EVERYTHING` with the metal plate's sounds, has no plate. The
  sound ids are the `SoundEvents` fields' usual names, **Inferred**: `SoundEvents` is not
  dumped; all twelve ids are in `reports/registries.json`'s `sound_event`.)
- Entities and what they press (`EVERYTHING` / `MOBS`), with the rule that the box must
  meet the touch box strictly (`AABB.intersects`, strict, AABB.txt 923–970 in the N5
  dump; **Inferred** that the section's test is `intersects`, `EntitySection` not
  dumped; 5.6 has the order of the query):

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

- `POWER` 0–15 (default 0); `maxWeight` from the constructor (7–27): 15 for the light
  plate (set `gold`, `Blocks.lambda$static$250`, `bipush 15`) and 150 for the heavy
  (set `iron`, `lambda$static$251`, `sipush 150`), as the repo's `plate_weight` has
  them.
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
(0.4, 0.7), `TRIPWIRE_DETACH` (0.4, `1.2f / (nextFloat() × 0.2f + 0.9f)`, a draw) with
their game events; `notifyNeighbors` (564–587). Nothing in the hook depends on entities.

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
- **Block set data**: a plate's sensitivity (`MOBS` or `EVERYTHING`) and its click
  sounds from the block, in `clustine-data`, by the table of 6.2 (poplar included).
- **Shears**: `DISARMED` set before a tripwire is broken by a player holding shears.

## 7. The crafter (optional for N6)

Read: `CrafterBlock` (CrafterBlock.txt, 611 lines) and `CrafterBlock$1` (its switch over
`Direction`), `CrafterBlockEntity` (552) and `CrafterBlockEntity$1` (its
`ContainerData`), `CrafterMenu` (its slots, `setSlotState`, `quickMoveStack`,
`refreshRecipeResult`), `ServerGamePacketListenerImpl.handleContainerSlotStateChanged`
(ServerGamePacketListenerImpl.txt 2824–2861) and `HopperBlockEntity.getContainerAt`/
`addItem` (4.18), `CrafterSlot`, `NonInteractiveResultSlot`, `ResultContainer` and
`ServerboundContainerSlotStateChangedPacket` (second dump). Not dumped: `RecipeCache`,
`CraftingInput`, the recipe classes, `ItemStack.onCraftedBySystem`.

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
level.setBlock(pos, state.setValue(CRAFTING, true), 2)    // the tick's state, with its TRIGGERED
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
  `CrafterSlot`).
- `CrafterSlot` (CrafterSlot.txt): `mayPlace(stack)` is `!menu.isSlotDisabled(index) &&
  Slot.mayPlace(stack)`; `setChanged()` is `Slot.setChanged()` then
  `menu.slotsChanged(container)` (the shown result refreshed). The result slot is a
  `NonInteractiveResultSlot` (NonInteractiveResultSlot.txt) over a `ResultContainer`:
  `mayPickup`, `mayPlace` and `allowModification` false, `tryRemove` empty, `safeTake`
  and `remove` give `EMPTY`, `safeInsert` gives the stack back, `onTake` and
  `onQuickCraft` do nothing, `isFake` true: a click on slot 45 changes nothing.
  `ResultContainer` (ResultContainer.txt) is one slot, `stillValid` true, `setChanged`
  nothing, and keeps the `RecipeHolder` used (`setRecipeUsed`/`getRecipeUsed`).
- `handleContainerSlotStateChanged(packet)` (ServerGamePacketListenerImpl.txt 2824–2861):
  not a spectator, `packet.containerId == player.containerMenu.containerId`, the menu is a
  `CrafterMenu` whose container is a `CrafterBlockEntity`: `be.setSlotState(packet.slotId,
  packet.newState)`. The change reaches the client as a `container_set_data` from the
  menu's `broadcastChanges` (section 3). The packet (serverbound
  `container_slot_state_changed`) is a record `(slotId, containerId, newState)` written
  as `slotId` (VarInt), `containerId` (`FriendlyByteBuf.writeContainerId`, the codec of
  `ByteBufCodecs.CONTAINER_ID`, 9.3), `newState` (boolean)
  (ServerboundContainerSlotStateChangedPacket.txt 9–55): **the slot comes first**.
- `CrafterMenu.setSlotState` (157–177) is the client's path (it calls
  `broadcastChanges` on its own menu); `isPowered()` reads data slot 9.

### 7.5 Draws

A craft draws only where something is dropped: per dropped stack, `getContainerAt`'s
possible `nextInt` (container entities in front, when no block container) and then
`spawnItem`'s seven level draws (4.3) and the item's own six. A craft that fills a
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
`position_source_type`); from the second dump `GameEvent` (and `$Context`,
`$ListenerInfo`), `GameEventDispatcher`, `GameEventListener` (and `$DeliveryMode`,
`$Provider`), `VibrationSelector` (and `$VibrationEvent`), `VibrationInfo`,
`CalibratedSculkSensorBlockEntity` and its `$VibrationUser`, `ItemEntity`,
`FallingBlockEntity`, `LevelChunk`, `Level`, `ServerLevel`, `Block`, `BlockItem`,
`BucketItem`, `BoneMealItem`, `ComposterBlock`, and the n5 dump's `BlockGetter`
(`isBlockInLine`). Still not dumped: `EuclideanGameEventListenerRegistry`,
`BlockPositionSource`, `EntityPositionSource`, `VibrationParticleOption`,
`SculkShriekerBlock(Entity)`, `LevelAccessor` (the `gameEvent` overloads),
`ClientboundLevelParticlesPacket`, and the blocks already built that post events
(levers, buttons, doors, pistons, note blocks, bells). What those say is marked
**Inferred** below and listed in section 12.

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
   N6 has to go back and post them. Breaking a block by hand posts `block_destroy`
   (`Block.playerWillDestroy`, Block.txt 1172–1200: context `(player, old state)` at
   the position) and placing one `block_place` (`BlockItem.place`, BlockItem.txt 55–,
   at 239–250: `(player, placed state)`, after the place sound); an item or a falling
   block landing posts `hit_ground` (8.1).
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
   (VibrationSelector.txt 98–123: held tick `< now`), so the sensor's ticker takes it on the
   next tick, sets the travel time to `Mth.floor(distance)` (User.txt 26–30), sends
   the vibration particle, and receives when the travel time reaches 0 after the
   decrement of that same tick or a later one: received at tick `T + max(1,
   floor(d))` for an event at tick `T` at distance `d`. While the sensor holds a
   vibration it hears nothing else (Listener.txt 40–44).
7. **Several events in one tick: the closest wins, then the higher frequency**
   (`VibrationSelector.shouldReplaceVibration`, VibrationSelector.txt 45–97, now read:
   confirmed): a later candidate of the same tick replaces the held one if it is
   strictly closer, or as close and of a strictly higher frequency (at equal distance
   and frequency the first is kept); a candidate of another tick than the held one
   never replaces it. The distance compared is the float `eventPos.distanceTo(
   listenerPos)` (`fcmpg`/`fcmpl`, so a NaN distance never replaces).
8. **Power is by block distance; the comparator reads the frequency.** On receipt the
   power is `max(1, 15 − floor(15.0 / radius × d))` (VibrationSystem.txt 43–59) with
   `d = (float) sqrt(BlockPos.containing(eventPos).distSqr(sensorPos))`
   (Listener.txt 131–138), `r = getListenerRadius()`: 8, or 16 for a calibrated
   sensor (CalibratedSculkSensorBlockEntity$VibrationUser.txt 19–22; the plain user
   passes its own radius, SculkSensorBlockEntity$VibrationUser.txt 92–95). The comparator
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
    dispatcher looks for listeners in every loaded section within the event's
    notification radius (16 for all but `jukebox_play`/`jukebox_stop_play` 10 and
    `shriek` 32, GameEvent.txt 151–157, static 189–) and the sensor tests its own
    radius (8 or 16) by block
    distance; the listener reads the source entity's flags (spectator, shift,
    `dampensVibrations`), the affected block's tags and, for occlusion, up to six
    straight lines of blocks between the two positions (8.5). Section 11 of this
    brief has what an event effect has to carry.
11. **Calibrated sensors are now fully read** (8.10): `facing` from the player's
    horizontal direction, active for 10 ticks, no signal out of the side named by
    `facing`, radius 16, and a frequency filter: while the signal into its back (the
    side opposite `facing`) is above 0, only events of exactly that frequency are
    heard. Optional for N6; the plain sensor does not need it.
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
post at `Vec3.atCenterOf(pos)` (**Inferred**: they are defaults of `LevelAccessor`,
still not dumped; `Level` and `ServerLevel` do not override them). On the server they
all reach `ServerLevel.gameEvent(e, Vec3, Context)` → `gameEventDispatcher.post(e, pos,
ctx)` (ServerLevel.txt 2372–2380). The context (`GameEvent$Context`) is a source entity
(or null) and an affected block state (or null).

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
| `fluid_place`, `fluid_pickup` | `DispenseItemBehavior$13` 104; `$4` 53, `$9` 14 | buckets dispensed | null entity (`aconst_null` before the post: read) |
| `entity_place` | `DispenseItemBehavior$6` 80, `SpawnEggItemBehavior` 65 | armour stand, spawn egg dispensed | |
| `block_place` | `DispenseItemBehavior$7` 48, `$8` 46, `FlintAndSteelDispenseItemBehavior` 51 | a block placed by a dispenser; fire lit | |
| `block_change` | `FlintAndSteelDispenseItemBehavior` 76 | a block lit (candle, campfire, TNT?) | |
| `shear` | `ShearsDispenseItemBehavior` 101, 155 | beehive or entity sheared | |
| `sculk_sensor_tendrils_clicking` | `SculkSensorBlock.activate` 380–452 (63–69) | a sensor activates | the vibration's entity, at the sensor; frequency 0 and not in `#vibrations`: only shriekers hear it |
| `resonate_1`…`resonate_15` | `SculkSensorBlock.tryResonateVibration` 453–504 | an amethyst block beside an activating sensor | `(entity, amethyst state)` at the amethyst block |

Posted by the other classes of the second dump: `block_destroy` by
`Block.playerWillDestroy` (Block.txt 1172–1200, `(player, state)`), `block_place` by
`BlockItem.place` (BlockItem.txt 239–250, `(player, placed state)`), `fluid_pickup` by
`BucketItem.use` (159) and `fluid_place` by `BucketItem.playEmptySound` (448),
`item_interact_finish` by `BoneMealItem.useOn` (43, 80), `block_change` by the
composter's `addLayer` (433) and `empty` (366), `entity_damage` by
`ItemEntity.hurtServer` (732–786, at 771: the damage source's entity, so an item hurt
by a TNT posts it with the TNT's owner or none), `entity_die` by `ServerPlayer.die`
(1777).

Not posted: a successful dispense of an item (the default behaviour posts nothing),
blocks broken by an explosion (only the one `explode`), a sensor's own state changes
(`setBlockAndUpdate` posts nothing), a falling block's landing set (only its
`hit_ground`). Arrows, snowballs, eggs and other projectiles post
`projectile_shoot`/`projectile_land` (N9). Items (`ItemEntity.getMovementEmission`
154–158) and falling blocks (FallingBlockEntity.txt 178) have `MovementEmission.NONE`
and post no steps, but `Entity.checkFallDamage` (3990–4042) is not gated by the
emission and neither class overrides it (read: `FallingBlockEntity` overrides only
`causeFallDamage`), so **an item or a falling block landing after a fall posts
`hit_ground`** (context: the entity and the block it lands on) and one entering water
`splash`. Clustine's items keep no fall distance today (`region/items.rs`);
`hit_ground` for items needs it. `PrimedTnt` has `MovementEmission.NONE`
(PrimedTnt.txt), so TNT makes no steps; it lands (`hit_ground`) as an item does.
Minecarts and boats have `EVENTS` (n5 `AbstractMinecart`, `AbstractBoat`): they post
steps and swims but play no step sound.

### 8.2 The dispatcher (`GameEventDispatcher`, `GameEvent`, `GameEventListener`, `LevelChunk`)

`GameEvent` is a record of one `int`, `notificationRadius` (GameEvent.txt 2–3, 131–149).
`register(name)` gives 16 (`DEFAULT_NOTIFICATION_RADIUS`, 151–157); the static block
(189–) registers every event so but `jukebox_play` and `jukebox_stop_play` (10) and
`shriek` (32). The registration order is the registry's (ids in 9.6).

`post(event, pos, ctx)` (GameEventDispatcher.txt 14–135):

1. `r = event.notificationRadius()`; `b = BlockPos.containing(pos)`; the section
   bounds `blockToSectionCoord(b.x − r)` … `(b.x + r)`, likewise y and z (14–96).
2. Loops (120–216): section x outermost, then z; the chunk by
   `getChunkSource().getChunkNow(x, z)` (a chunk not fully loaded is skipped); then
   section y innermost: `chunk.getListenerRegistry(y).visitInRangeListeners(event,
   pos, ctx, visitor)`, the results or-ed. The registry of a section is made on demand
   for a `ServerLevel` (`LevelChunk.getListenerRegistry` 287–310, an
   `EuclideanGameEventListenerRegistry`, lambda 1893–).
3. Within a section's registry (**Inferred**: `EuclideanGameEventListenerRegistry`
   still not dumped): each listener in the order registered; the listener's position
   (`BlockPositionSource`: the block's centre, **Inferred**) `p`; it is visited only
   if `BlockPos.containing(p).distSqr(b) ≤ listenerRadius²` (between block positions,
   so `dx² + dy² + dz² ≤ 64` in whole blocks for a sensor).
4. The visitor (lambda 167–194): a listener whose `getDeliveryMode()` is
   `BY_DISTANCE` is put in a list as a `GameEvent$ListenerInfo` (with
   `distanceToRecipient` a `distanceToSqr` as a double, ListenerInfo.txt); every other
   is handled at once with `handleGameEvent(level, event, ctx, pos)`. After the loops,
   if the list is not empty it is sorted by that distance (`Collections.sort`, stable,
   `Double.compare`) and each handled in turn (`handleGameEventMessagesInQueue`
   136–165). `GameEventListener.getDeliveryMode` defaults to `UNSPECIFIED`
   (GameEventListener.txt) and `VibrationSystem$Listener` does not override it, so
   **sensors are handled at once, in the visit order** (confirmed). For sensors the
   order does not matter: each has its own selector.
5. A listener is registered when its block entity is added to a chunk of a
   `ServerLevel` (`LevelChunk.addAndRegisterBlockEntity` 896–925 →
   `addGameEventListener` 1638–1658: `EntityBlock.getListener(level, be)`, registered
   in the section of the block entity's y) and for every block entity when a chunk
   is loaded (`registerAllBlockEntitiesAfterLevelLoad` 1628–, lambda 1739–1756); it is
   unregistered by `removeBlockEntity` (1128–1164 → `removeGameEventListener`
   1165–1197). An unloaded chunk takes its registries with it; `getChunkNow` no longer
   finds it.

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
   (false for every entity but an item: `ItemEntity.dampensVibrations`, ItemEntity.txt
   120–127, is the stack in the item tag `#dampens_vibrations` = `#wool`,
   `#wool_carpets`, `#wool_slabs`, `#wool_stairs`; `Entity.dampensVibrations` 3954 is
   false). So a dropped wool item falling makes no `hit_ground` a sensor hears.
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
`BlockGetter.traverseBlocks` does (n5 BlockGetter.txt 44–56, lambdas 1039–1075: the
predicate is tested on the block state alone, no shape; a match is a `BLOCK` hit at
`to`, the walk's end without one a `MISS`) and stops at the first that matches. The
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

`VibrationSelector` (VibrationSelector.txt, read): it holds at most one
`VibrationEvent(info, tick)` (`currentVibrationData`, an `Optional`).
`addCandidate(info, tick)` (28–44) replaces it when `shouldReplaceVibration` (45–97):
true if none is held; false if the held tick differs from `tick`; true if
`info.distance < held.distance`; false if it is greater; else true only if
`frequency(info) > frequency(held)`. `chosenCandidate(now)` (98–123) gives the held
info if its tick `< now`, else none. `startOver()` (124–130) empties it. Its codec
(lambda 140–158): `event` (lenient optional `VibrationInfo`), `tick` (long; written as
−1 when empty).

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
`VibrationInfo` (VibrationInfo.txt) is a record `(gameEvent, distance, pos, uuid,
projectileOwnerUuid, entity)`; its codec (static 279–, lambda 211–245): `game_event`
(`GameEvent.CODEC` = `RegistryCodecs.holder(Registries.GAME_EVENT)`, GameEvent.txt
377–379: the registry name, **Inferred**), `distance` (`Codec.floatRange(0,
3.4028235E38)`), `pos` (`Vec3.CODEC`, a list of three doubles), `source` (UUID,
lenient optional), `projectile_owner` (UUID, lenient optional); the entity is not
saved. The constructor from an entity (29–46) takes its UUID and, if it is a
`Projectile` with an owner, the owner's UUID (`getProjectileOwner`, 71–88).
`getEntity(level)` (89–99) is the held entity, else `level.getEntity(uuid)`;
`getProjectileOwner(level)` (100–116) the held entity's owner if it is a projectile,
else `level.getEntity(projectileOwnerUuid)`.

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
- `CalibratedSculkSensorBlockEntity` (type `minecraft:calibrated_sculk_sensor`)
  makes a `CalibratedSculkSensorBlockEntity$VibrationUser`, which extends the plain
  user (now read): `getListenerRadius()` 16 (19–22), so the power is
  `getRedstoneStrengthForDistance(d, 16)`; `canReceiveVibration` (24–50) first reads
  `back = getBackSignal(level, blockPos, state)` and refuses when `back ≠ 0` and
  `getGameEventFrequency(event) ≠ back`, then the plain user's test; `getBackSignal`
  (51–) is `level.getSignal(pos.relative(d), d)` with `d = facing.getOpposite()`:
  the signal coming in from the side opposite `facing` (the first reading had
  `facing` itself: wrong).

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
(the selector as read, 8.6). `stepOn` runs from `Entity.applyEffectsFromBlocks`
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
| 4 | `playerKnockback` | `Optional<Vec3>` | `Vec3.STREAM_CODEC.apply(ByteBufCodecs::optional)`: a boolean, then the three doubles when present (`ByteBufCodecs.optional`, ByteBufCodecs.txt 177–183, is `ByteBufCodecs$25`, which writes `writeBoolean(isPresent)` and then the value; that the method reference is `optional` is **Inferred**, `javap -c` not showing it) |
| 5 | `explosionParticle` | `ParticleOptions` | `ParticleTypes.STREAM_CODEC`: VarInt particle type id, then the type's options (none for `explosion` 30 and `explosion_emitter` 29) |
| 6 | `explosionSound` | `Holder<SoundEvent>` | `SoundEvent.STREAM_CODEC` (a holder: VarInt `id + 1` for a registered sound, 0 then an inline event; `entity.generic.explode` 703 → 704) (**Inferred** holder layout, as `sound` packets elsewhere in the edge) |
| 7 | `blockParticles` | `WeightedList<ExplosionParticleInfo>` | `WeightedList.streamCodec(c)` (WeightedList.txt, `streamCodec`): `Weighted.streamCodec(c)` through `ByteBufCodecs.list()`, so a VarInt count (`writeCount`, ByteBufCodecs.txt 185–220) and then the entries. Each entry is `Weighted`'s codec, **Inferred** the value then a VarInt weight (`Weighted` is not dumped); the value is `ExplosionParticleInfo.STREAM_CODEC`, **Inferred** the particle (as 5), a float scaling, a float speed (not dumped). The default list: `poof` (69) `0.5f, 1.0f`; `smoke` (72) `1.0f, 1.0f`; weights 1 (`WeightedList$Builder.add(E)`, 14–20) |
| 8 | `playSound` | `boolean` | `ByteBufCodecs.BOOL`: `source == null || !source.isSilent()` |

Sent per player (ServerLevel.txt 2640–2686) to every player of the level whose
`distanceToSqr(center) < 4096`; `playerKnockback` is that player's entry in
`hitPlayers` (a survival or adventure player, or a creative one not flying, within the
damage range) or empty.

**Primed TNT** (`minecraft:tnt`, entity type 136): spawned by `add_entity` (1) with data
0 (`PrimedTnt` does not override `getAddEntityPacket`; `Entity`'s, Entity.txt
10682–10689, builds `ClientboundAddEntityPacket(entity, serverEntity)`) and the initial
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

A block event is run at the end of the tick's block events phase and sent only when the
block's `triggerEvent` answers true, to every player of the dimension within 64 blocks of
the position's corner (`ServerLevel.runBlockEvents`, ServerLevel.txt 2717–2785); the
queue is a linked set, so an identical event queued twice in a tick goes once.

The barrel sends none: its `open` property changes (a block update, flags 3).

**Sounds** (`ClientboundSoundPacket`, built for other sounds; source `blocks`, at the
position given, the seed from `soundSeedGenerator.nextLong()`):

| Sound (id) | Where | Volume | Pitch |
|---|---|---|---|
| `block.chest.open` 349 / `block.chest.close` 347 (copper chests: `block.copper_chest.open` 406 / `.close` 405; weathered 408 / 407; oxidized 410 / 409) | single: block centre; double: the `right` half's centre + its connected direction × 0.5 | 0.5 | `nextFloat() * 0.1f + 0.9f` |
| `block.barrel.open` 141 / `block.barrel.close` 140 | block centre + facing's unit vector / 2 | 0.5 | `nextFloat() * 0.1f + 0.9f` |
| `block.shulker_box.open` 1469 / `.close` 1468 | block centre | 0.5 | `nextFloat() * 0.1f + 0.9f` |
| `block.chest.locked` 348 | the container's centre (a double chest: the pair's centre) | 1.0 | 1.0 |

**System chat, overlay** (`ClientboundSystemChatPacket`, overlay true):
`container.isLocked`
with the container's display name as its argument; `container.spectatorCantOpen` in red.

**Open screen** (`ClientboundOpenScreenPacket`, field by field in 3.10 on): container id
(1 to 100), menu type (`generic_9x3` 2, `generic_9x6` 5, `generic_3x3` 6, `crafter_3x3`
7, `hopper` 16, `shulker_box` 20), title (the custom name, or `container.chest`,
`container.chestDouble`, `container.barrel`, `container.shulkerBox`,
`container.dispenser`,
`container.dropper`, `container.hopper`, `container.crafter`; a vehicle's display name).

**Block entity data** saved (the store's and the chunk's form; never sent):
`lock` (when not `NO_LOCK`), `CustomName` (a text component, when named),
`LootTable` (identifier string) and `LootTableSeed` (long, when not 0) while a table is
set, else `Items` (list of `{Slot: byte, id, count, components}` for non-empty slots;
always written by chests and barrels, omitted when empty by shulker boxes),
`components` (always). Block entity type ids: `chest` 1, `trapped_chest` 2,
`ender_chest` 3 (its contents are the player's), `dispenser` 5, `dropper` 6, `hopper` 18,
`shulker_box` 24, `barrel` 26,
`crafter` 42.

**Game events** (section 8): `container_open` (11) and `container_close` (10), at the
block's centre, with the opener (the counter's first open, last close; the shulker
box's), or with no entity (a recheck's correction); for vehicles at the entity's
position.

### 9.3 Menus and clicks (3.10 to 3.21)

All ids from `generated/reports/packets.json` (play). `CONTAINER_ID` is
`ByteBufCodecs.CONTAINER_ID` (`ByteBufCodecs$24.txt`: `FriendlyByteBuf.readContainerId` and
`writeContainerId`; `FriendlyByteBuf` is not dumped, so that this is a VarInt is still
**Inferred**, as the edge's `SetContainerContent` writes it today and as SteelMC reads
it). Other codecs, read in `ByteBufCodecs` and its nested classes: `idMapper` is a VarInt
(`$31`); `registry(key)` and `holderRegistry(key)` are a VarInt registry id (`$32`;
`holderRegistry` is `registry(key, Registry::asHolderIdMap)`, ByteBufCodecs.txt 360–384,
so a plain id, not the "id + 1 or inline" of `holder`, `$33`), and an unknown id fails the
decoding (`byIdOrThrow`); `optional(codec)` is a Boolean, then the value if true (`$25`);
`map(factory, k, v, max)` is a VarInt count (`writeCount`/`readCount` with the maximum,
refused above it), then each key and value, the map made with room for `min(count,
65536)` (`$28`).
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
| | `containerInput` | `ContainerInput.STREAM_CODEC`: VarInt (`idMapper`) | 0 `PICKUP` … 6 `PICKUP_ALL`; any other id reads as `PICKUP` (`ByIdMap` strategy `ZERO`) |
| | `changedSlots` | `ByteBufCodecs.map(Int2ObjectOpenHashMap::new, SHORT→int, HashedStack.STREAM_CODEC, 128)`: VarInt count (refused above 128, `readCount`, n5 ByteBufCodecs.txt 185–202), then per entry Short slot and `HashedStack` | the client's view after its prediction |
| | `carriedItem` | `HashedStack` | |
| `HashedStack` | present | Boolean (`ByteBufCodecs.optional`) | false = `EMPTY` |
| | item | `ByteBufCodecs.holderRegistry(ITEM)`: VarInt registry id | HashedStack$ActualItem.txt 88–100 |
| | count | VarInt | |
| | added components | VarInt count ≤ 256, then per entry: component type (`ByteBufCodecs.registry(DATA_COMPONENT_TYPE)`, VarInt id) and Int (4 bytes, big-endian): the component's hash (3.16) | HashedPatchMap.txt 150–168 |
| | removed components | VarInt count ≤ 256, then component type VarInt each | |
| `container_close` (19) | `containerId` | `CONTAINER_ID` | ServerboundContainerClosePacket.txt 16–32 |
| `container_button_click` (17) | `containerId`, `buttonId` | `CONTAINER_ID`, VarInt (ServerboundContainerButtonClickPacket.txt 72–80) | ignored in N6 |
| `container_slot_state_changed` (20) | `slotId`, `containerId`, `newState` | VarInt, `readContainerId`, Boolean, in that order (ServerboundContainerSlotStateChangedPacket.txt 11–22, 38–55) | the crafter |
| `set_creative_mode_slot` (57) | `slotNum`, `itemStack` | Short, then `ItemStack.OPTIONAL_UNTRUSTED_STREAM_CODEC` (ServerboundSetCreativeModeSlotPacket.txt 81–90; components length-prefixed, as `Reader::untrusted_item_stack` reads) | already decoded by the edge |

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
| `set_cursor_item` (98) | `contents` | `ItemStack.OPTIONAL_STREAM_CODEC` (ClientboundSetCursorItemPacket.txt 61–67) | the carried stack, sent by the synchroniser's `sendCarriedChange` (ServerPlayer$1.txt 78–89); no container id, no state id |
| `set_player_inventory` (110) | slot, stack | as the edge has it | not used by the menus; the edge's way for region-made changes |

When each goes:

- `open_screen` then `container_set_content` (state id incremented) and one
  `container_set_data` per data slot: at `openMenu` (3.10).
- `container_set_slot` (state id incremented each): from `broadcastChanges` for each slot
  whose
  real stack no longer matches the remote slot: after every click, at every player tick, and
  the other callers of 3.15.
- `set_cursor_item`: when the carried stack does not match the remote carried one (the
  state id does not move).
- `container_set_data` from `broadcastChanges`: when a data slot's value differs from the
  remote one (the state id does not move).
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

**Sounds** (`ClientboundSound`, source `blocks`, volume 1, pitch 1). The packet's seed is
**not** a draw of the level's random: `Level.playSound` takes it from the level's own
`soundSeedGenerator` (`RandomSource.createThreadSafe()`, Level.txt 96–97; the
`nextLong` at 746–762 and in the other overloads, 685–808) and passes it to
`ServerLevel.playSeededSound` (ServerLevel.txt 2222–2328). So sounds never shift the
level's stream; Clustine may seed them from anything: `entity.tnt.primed` at the
PrimedTnt's position (dispensed TNT, `TntBlock.prime`); `item.honeycomb.wax_on` at the
front block; `item.bottle.empty` at the dispenser (water potion to mud); `block.beehive.
shear` at the beehive (shears); `item.shears.snip` at the sheared entity's block
position (leashes cut, 4.16); `item.bucket.empty`/`item.bucket.empty_lava` at the block
(volume 1, pitch 1; `BucketItem.playEmptySound`), `block.fire.extinguish` (volume 0.5,
pitch `2.6 + (f1 − f2) · 0.8`, two **level** `nextFloat`, where water evaporates) with
eight `large_smoke` particles at the block's corner (offset 1, speed 0, `ALTERNATIVE`);
the shulker box's place sound is its sound type's (`(volume + 1) / 2`, `pitch · 0.8`,
`BlockItem.place`, 4.15).

**Particles** (`ClientboundLevelParticles`): `splash`, five packets of count 1, offset 0,
speed 1, at `(pos.x + nextDouble(), pos.y + 1, pos.z + nextDouble())` of the dispenser
(water potion to mud).

**Block states**: `dispenser`/`dropper` `facing` (6) × `triggered` (2); `crafter`
`orientation` (12, in `reports/blocks.json`'s order: `down_east`, `down_north`,
`down_south`, `down_west`, `up_east`, `up_north`, `up_south`, `up_west`, `west_up`,
`east_up`, `north_up`, `south_up`: the front first, then the top) × `triggered` ×
`crafting`. No block events.

**Block-entity data** (chunk NBT; none of these is sent to the client in the chunk
packet: neither `getUpdateTag` nor `getUpdatePacket` is overridden by the dispenser's,
dropper's or crafter's entities or their bases, so `BlockEntity`'s null packet and empty
tag stand, 3.0 item 14): `dispenser`/`dropper`: `CustomName`, `lock`, `Items` (`Slot`
byte + the item) or `LootTable` + `LootTableSeed` (the seed written only when not 0,
`trySaveLootTable`). `crafter`: the same plus `crafting_ticks_remaining`
(int), `disabled_slots` (int array), `triggered` (int).

**Menus**: `generic_3x3` for dispensers and droppers (9 slots + 36), `crafter_3x3` for the
crafter (9 + 36 + the result slot 45, and ten data slots: 0–8 the slot states, 9
`triggered`, each sent as `container_set_data(id, index, value)`). Serverbound
`container_slot_state_changed`: slot id (VarInt), then the container id
(`writeContainerId`, as every menu packet's, 9.3), then the new state (boolean)
(ServerboundContainerSlotStateChangedPacket.txt 9–55; the earlier guess had the
container id first).

**Entities made** (their spawn packets as for any): item entities (motion as 4.3, data
the stack), `tnt` (fuse 80, the block state data: section 1), boats and chest boats,
minecarts of six kinds, armour stands; equipment changes on players (their inventory
slot) and stands (`set_equipment`).

### 9.5 Hoppers, plates and tripwire (sections 5 and 6)

Hoppers, plates and tripwire add no packet of their own:

- **Block changes**: `ENABLED` of a hopper (flag 2), `POWERED` / `POWER` of plates (flag
  2), `POWERED` of tripwire (`setBlockAndUpdate`, flag 3), `DISARMED` (flag 260): each
  is a `block_update` (or a section update) as today.
- **Sounds**: plates' `pressurePlateClickOn`/`Off` of their block set (the table of
  6.2; `sound`, source `BLOCKS`, volume 1, pitch 1, the seed); hooks' four sounds (6.5).
- **Hopper block entity**: no `block_entity_data` (`getUpdatePacket` is
  `BlockEntity`'s, null, BlockEntity.txt 525–528); in a chunk's data its entry carries
  `getUpdateTag`, `BlockEntity`'s empty compound (BlockEntity.txt 530–535), which the
  hopper does not override. Its contents travel only in the menu's packets (section 3).
- **Composter fed or emptied by a hopper**: a `block_update` of `LEVEL` (flag 3); a
  `level_event` 1500 at its position with data 1 when a layer was added, 0 when not
  (the fill's sound and particles, **Client**), sent for each item composted; at
  `LEVEL` 7 a scheduled tick 20 later turns it to 8 with the sound
  `block.composter.ready` (`BLOCKS`, 1, 1). Emptying sends no level event.
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
`event_delay` int ≥ 0, always written). The inner codecs are now read (8.9, 8.6):
`VibrationInfo` writes `game_event` by `RegistryCodecs.holder(GAME_EVENT)`, the
registry name (`"minecraft:step"`; **Inferred**: `RegistryCodecs` not dumped), not the
id; `distance` a float in [0, 3.4028235E38]; `pos` a list of three doubles;
`source` and `projectile_owner` UUIDs (int arrays), each left out when absent; the
selector's `tick` is −1 when it holds nothing.

**Game event ids** (`registries.json`, `minecraft:game_event`, for the wire; the saved
`VibrationInfo` uses the names; radius 16 for each but `jukebox_play` and
`jukebox_stop_play` 10 and `shriek` 32, GameEvent.txt static 189–): block_activate 0,
block_attach 1, block_change 2, block_close 3, block_deactivate 4, block_destroy 5,
block_detach 6, block_open 7, block_place 8, bounce 9, container_close 10, container_open
11, drink 12, eat 13, elytra_glide 14, entity_damage 15, entity_die 16, entity_dismount
17, entity_interact 18, entity_mount 19, entity_place 20, entity_action 21, equip 22,
explode 23, flap 24, fluid_pickup 25, fluid_place 26, hit_ground 27, instrument_play 28,
item_interact_finish 29, item_interact_start 30, jukebox_play 31, jukebox_stop_play 32,
lightning_strike 33, note_block_play 34, prime_fuse 35, projectile_land 36,
projectile_shoot 37, sculk_sensor_tendrils_clicking 38, shear 39, shriek 40, splash 41,
step 42, swim 43, teleport 44, unequip 45, resonate_1 … resonate_15 46 … 60.

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

**A TNT burnt by fire** (`FireBlock.checkBurnOut`, 1.1), all the level's: `nextInt(300)`
(`nextInt(250)` above or below; 50 less with `increased_fire_burnout`); when it is
under 100, `nextInt(age + 10)`; when that is under 5 and it does not rain there,
`nextInt(5)` (the new fire's age); then the priming's `nextDouble` (and the entity's own
UUID draws). No fuse draw: the fuse is 80.

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
   the item's own); a container's spill, at the set (per slot three `nextDouble`; per
   part one `nextInt(21)` and six `nextDouble` for the three `triangle`s, two each,
   RandomSource.txt 77–88; the item's own draws); a TNT's priming (as above:
   `nextDouble`, `nextInt(20)`). Within one block the order is `spawnAfterBreak`'s
   draws (experience from ores, a silverfish from an infested block), the loot, the set
   (its updates and the spill), then `wasExploded` (2.6).
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
  split's size), then of **the new item's own random** (not the level's) the
  `ItemEntity` constructor's two `nextFloat()` (bob, yaw) and two `nextDouble()` (a
  motion that is then replaced; ItemEntity.txt 33–98), then of the level's random
  `triangle` × 3 (x, y, z motion), each **two** `nextDouble()`: `triangle(a, b) = a + b
  · (nextDouble() − nextDouble())` (RandomSource.txt 77–89). So six level draws of
  motion per stack, after its `nextInt(21)`. An unopened loot table is first rolled by
  `LootTable.fill` with the seed's random (not read).
- **A creative player breaking a non-empty shulker box**: only the new item's own
  draws (two `nextFloat()`, two `nextDouble()` of its own random), at the block's
  centre; none of the level's.
- **Unpacking a loot table** (opening, reading, pulling, breaking): `LootTable.fill`'s
  rolls and its shuffle over the empty slots, all from the random of the seed
  (**Inferred**: `LootTable.fill` is not read; the seed is `LootTableSeed`,
  RandomizableContainer.txt 102–168). With a seed of 0 they come from the level's random,
  which
  the oracle cannot repeat.
- The shulker box's animation and entity push draw nothing.

### 10.3 Menus and clicks (3.10 to 3.21)

- No rule of a menu draws a random number. The `quickcraftSlots` set is a `HashSet` whose
  iteration order follows the slots' identity hashes; the result does not depend on it
  (3.14.1), so a port may use the slots' order.
- Items thrown out of a menu (`PICKUP`/`QUICK_MOVE` at −999, `THROW`, a `SWAP` whose
  stack does not fit back, and a carried stack dropped at closing by a removed or
  disconnected player) are `Player.drop(stack, includeThrower, prediction)` →
  `LivingEntity.drop` → `createItemStackToDrop(stack, false, includeThrower)` (n5
  LivingEntity.txt 1657–1695): the N2 throw from the eyes, with its draws from the
  player's random, in the order the items are thrown (`THROW` with button 1 over a result
  slot throws several, one after another). The closing drop (`includeThrower` false) is
  the same throw without the thrower recorded: `createItemStackToDrop` is
  `LivingEntity`'s (n5 LivingEntity.txt 9318–), overridden neither by `Player` nor by
  `ServerPlayer`, whose `drop` (ServerPlayer.txt 4985–) only adds the statistics.
- `placeItemBackInInventory` (Inventory.txt 724–784) draws only when no slot can take the
  rest, which it then drops with `player.drop(stack, false, prediction)`: the same throw.

### 10.4 Dispensers, droppers and the crafter (sections 4 and 7)

**A dispenser's tick**, in order (the level's random unless "own"):

1. `unpackLootTable(null)` where a loot table is set: no draw of the level's random in
   `RandomizableContainer.unpackLootTable` (RandomizableContainer.txt 102–167); the
   loot's draws are `LootTable.fill`'s with the saved seed (not dumped: what a seed of 0
   draws from).
2. `getRandomSlot`: one `nextInt(n)` per non-empty slot in slot order, `n` = 1, 2, 3, …
   (DispenserBlockEntity.txt 37–70).
3. The behaviour:
   - **drop** (default, and every fallback): the new `ItemEntity`'s own draws (two
     `nextLong` for the UUID, `nextFloat` bob, `nextFloat` yaw, two `nextDouble` motion:
     n4 8.4), then the level's `nextDouble` (speed) and three `triangle`, each two
     `nextDouble` (`a + b · (d1 − d2)`, RandomSource.txt 77–88): **7 level draws**. A
     fallback that dispenses twice (4.0 item 4)
     still drops once.
   - **remainder that does not fit**: another drop (7 level draws and the item's own six).
   - **TNT** (`$6`) and **flint and steel on TNT** (`TntBlock.prime`): the PrimedTnt's
     own UUID draws, then **one level `nextDouble`** (its direction). The sound's seed is
     the level's separate `soundSeedGenerator` (Level.txt 96–97, 746–762), not the
     level's random: no sound in this list shifts the level's stream.
   - **armour stand**: one level `nextFloat` (the discarded yaw, EntityType.txt 221–316),
     the stand's own UUID draws.
   - **boat, minecart**: their own UUID draws only.
   - **water potion to mud**: ten level `nextDouble` (five particle positions, x then z).
   - **honeycomb, beehive shear, glass bottle on a hive**: no level draw; the hive's
     `dropHoneycomb` → `Block.popResource` (n4 8.3's draws) for N7's case.
   - **flint and steel making fire**: fire's `onPlace` schedules its tick with
     `30 + nextInt(10)`, one level draw (`FireBlock.onPlace`, FireBlock.txt 818–845);
     `hurtAndBreak` draws nothing for a plain item (ItemStack.txt 744–786; only
     `EnchantmentHelper.processDurabilityChange`, for an enchanted one, may: not dumped).
   - **full bucket where water evaporates**: two level `nextFloat` (the extinguish
     sound's pitch, BucketItem.txt 241–276), before the particles.
   - **bone meal** (if built): `growCrop` is the block's `isBonemealSuccess` and
     `performBonemeal` with the level's random (each block's own draws, not dumped);
     `growWaterPlant` at a water source: per attempt `i` (0–127), `4 · (i / 16)` level
     `nextInt(3)` for the walk, then in a coral biome one `nextInt(4)` (and on success a
     `getRandomElementOf`), and at seagrass one `nextInt(10)` (BoneMealItem.txt 145–357).
   - **equipment, glowstone, shulker box, skull/pumpkin, empty bucket, other full
     buckets, shears on leashes**: no level draw (sounds are seeded apart, above).
   - **projectiles** (N9): `Projectile.shoot`'s inaccuracy (`triangle`, the projectile's
     own random, **Inferred**: `Projectile` not dumped); **spawn eggs** (N7): the mob's
     `finalizeSpawn`.

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
     the facing block, `level.random.nextInt(count)` (one draw; with one entity too:
     the hopper makes the call whatever the count, HopperBlockEntity.txt 939–981; that
     the level's random advances on `nextInt(1)` is **Inferred**, `RandomSource.nextInt`
     being abstract, RandomSource.txt 52, and `LegacyRandomSource` not dumped);
     when the target is a composter below `LEVEL` 7, composting the item draws what its
     `compostable` layers draw (a chance, from the loot context's random, **Inferred**
     the level's, 5.6);
  2. when not full, `suckInItems` → `getContainerAt(block above)`: the same, for the
     block above.
- **Item entity meeting a hopper** (`HopperBlockEntity.entityInside` → `tryMoveItems`):
  draw 1 above when ejecting; the take from the entity draws nothing.
- **Hopper cart** (`tryConsumeItems` → `HopperBlockEntity.suckInItems`): draw 2 above for
  the position `containing(x, y + 1.5, z)` (a container entity centred there); the
  fallback over item entities draws nothing.
- **Loot tables**: a hopper reading a source with a loot table unpacks it
  (`LootTable.fill` with the `LootTableSeed`, its own random, not the level's;
  RandomizableContainer.txt 102–167), before anything else of that source.
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
  across the line is read from the copy (ADR-0026 section 10). A fire across the line
  from a TNT burns it through the fire's own effect (ADR-0025 section 4, "fire
  spreading, a block removed by a rule"): `checkBurnOut` removes or replaces the TNT
  and then primes it, so the effect has to carry "burnt" and the holder primes it after
  the conditional set succeeds; the priming's draws are then the holder's.
- An exploded container across the line spills on the far side: the spill happens
  inside the far side's set (`LevelChunk.setBlockState`), from the block entity the
  holder alone has.
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
- **A trapped chest's signal follows its openers' count**, and its change updates the
  neighbours of the chest and of the block below (3.3): when either lies across a line
  the update crosses as an update effect does today; the count itself is the chest's
  region's, so a trapped chest is pressed only by the players its own region runs.
- **The ender chest's contents are the player's**, not the block's: its menu reads and
  writes only the player's own data (and the active chest's openers' count), so of all
  containers it alone could open across a line without moving items between regions;
  the lid and sound are still the chest's region's (**Inferred**, for the plan).
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

- **Opening across a line.** A chest, barrel, dispenser, dropper, hopper, crafter or
  shulker box opens by `useWithoutItem` on its block, which ADR-0026 section 7 refuses on
  land the player's region does not hold. To open it, the menu would need the container's
  holder to run the clicks (it changes the container's slots) and the player's region to
  change the inventory, in one step, which two regions cannot do without lending.
  Proposal: keep the refusal (the click does nothing) and name it; where regions follow
  players they merge when players work at a line (ADR-0028), so the seam remains only at
  pinned lines. A player sees it within minutes only if a pinned line runs through a
  storage room: the plan should say so.
- **A double chest across a line** (its halves in two regions): the `CompoundContainer`
  reads both halves; opening needs both in the player's region (3.1–3.9 say what the
  block does); with one half foreign, refuse as above (**Inferred** proposal: open
  neither, rather than a single chest's menu, which would show the half differently from
  the game).
- **A player whose region changes with a menu open** (riding a minecart or a boat,
  pushed, falling; a creative player cannot walk with a screen open, **Client**): in the
  game the menu stays while `stillValid` (within reach + 4 of the block) holds. Across a
  transfer the old region keeps the container and the new one the inventory. Proposal:
  the leaving region closes the menu as the game's `closeContainer` does (the carried
  stack put back into the inventory before the inventory goes with the transfer,
  `stopOpen` run so the lid closes and the openers' count falls), and the edge sends
  `container_close`. A named difference that shows only to a player riding with a chest
  open.
- **A container entity crossing with a menu open on it** (a chest or hopper minecart, a
  chest boat, N5's): in the game the menu lives while the cart is within reach + 4
  (`ContainerEntity.isChestVehicleStillValid`, N5 brief 3.18). Proposal: a cart's
  transfer closes every menu open on it in the region it leaves, as `stillValid` failing
  would; viewers in the cart's new region then reopen it.
- **The carried stack and duplication.** A click moves items between the container and
  the inventory; both must change in the same commit of one region, as they do when the
  region holds both. The carried stack is not durable in the game (lost if the server
  stops); here it is lost if the region restarts with a menu open, as in the game, but at
  a transfer, a split or a merge it must be put back into the inventory first (closing
  the menu), never copied.
- **Two viewers of one container in two regions** cannot happen with the refusal above;
  with it, the openers' count (and the lid) is the holder's alone.

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
  (`addItem`, its own cooldown hand-over with `k = 0`) and returns what does not fit as
  an effect back, which the hopper puts back into the same slot or, if that is no longer
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
- **The composter across a line** is block state only (no block entity), so a hopper
  feeding it or emptying it changes another region's block: the same options as above,
  with option (b)'s effect the composter's own rule run by its holder (compost one item,
  or hand the bone meal back as an offer). Its draw (the compostable chance) is the
  holder's then.
- **The entity queries' reach**: the sections searched extend 2 blocks beyond the box in
  x and z and 4 below it (5.6), but only entities whose boxes meet the box are kept, so
  what crosses is set by the boxes, not by the sections.
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
  loaded section within the event's radius (16 for every event a sensor hears,
  GameEvent.txt 151–157) of the event's block, so a post
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
  `handleGameEvent` on its own land and copies. In the game the dispatch reads only
  chunks that `getChunkNow` finds (GameEventDispatcher.txt 145–163), and a border copy
  is no `LevelChunk` with a listener registry: a sensor across the line is found only
  through the holder. The occlusion's segments reach at most
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
This is what is still unread after the second dump (`more.txt`); what that dump
settled is said where it is used. Asked for by more than one part: `LootTable.fill` (how
container loot is spread over slots, what a seed of 0 means), `EntitySection`/
`ClassInstanceMultiMap`/`LevelEntityGetterAdapter` (the order of entities within a
section and the per-entity box test), the fluid classes `WaterFluid`/`LavaFluid`/
`EmptyFluid` (fluid resistance), `FriendlyByteBuf` (whether the container id is a
VarInt), `LevelEvent` (level event names), `LevelAccessor`'s `gameEvent` defaults (the
position a block's post uses) and `javap -v` dumps of `Blocks`, `ChestBlock`,
`HopperBlock`, `EntitySelector`, `ServerPlayer$1` and `HashOps` (the method references
behind lambdas).
### 12.1 TNT and the explosion (sections 1 and 2)

The second dump settled the rest of what this part needed (`BlockBehaviour`,
`Block`, `Blocks`, `FluidState`, `FireBlock`, `GameRules`, `DamageSources`,
`ItemEntity`, `FallingBlockEntity`, `WeightedList`, `ByteBufCodecs`,
`Level$ExplosionInteraction`, `ServerLevel$1`, `ClipContext`, `EntitySectionStorage`,
`LevelEntityGetter`, `RandomSource`, `Attributes`, `ItemStack`, `LevelChunk`). Still
not read:

- Not read: `WaterFluid`, `LavaFluid`, `EmptyFluid` (or `Fluid.getExplosionResistance`
  in each), needed for the fluids' resistance (100, 100, 0 from SteelMC's extraction),
  which decides that a waterlogged block resists 100.
- Not read: `InfestedBlock` (its constructor), needed for the infested blocks'
  resistance (**Inferred** 0.75 from 1.21; `Blocks` gives them none); and a `javap -v`
  of `Blocks` (its bootstrap methods), needed to tie each lambda of
  `ColorCollection.registerBlocks` and of the weathering copper families to its family
  (the values are read; which family is which is matched by sound and value). The
  datagen's extract of `getExplosionResistance()` per block would settle both and is the
  recommended source.
- Not read: `Weighted` (its `streamCodec`), `ExplosionParticleInfo` and `Vec3$1`, needed
  for the order of the weight and the value in the explode packet's block particles,
  the particle info's two floats, and `Vec3.STREAM_CODEC` (**Inferred** three doubles);
  and a `javap -v` of `ClientboundExplodePacket`, to confirm that the knockback's
  operation is `ByteBufCodecs::optional`.
- Not read: `EntitySection`, `ClassInstanceMultiMap` and `LevelEntityGetterAdapter`,
  needed for the order of entities within one section and the box test there
  (**Inferred** insertion order and a strict `AABB.intersects`): which of two entities
  hurt by one explosion is handled first.
- Not read: `ItemUtils.onContainerDestroyed`, needed for where and with which draws a
  destroyed shulker box item spills its contents (matters only once stacks carry
  components).
- Not read: the block classes that override `onExplosionHit` (buttons, levers, bells,
  doors, trapdoors, gates, candles and the like, for `canTriggerBlocks`), needed only
  for wind charges (N9); TNT never triggers blocks.

### 12.2 Container blocks (3.1 to 3.9)

- Not read: `LootTable.fill` (and `getAvailableSlots`, `shuffleAndSplitItems`), needed
  for how a container's loot is spread over its slots and which random the seed gives
  (**Inferred**: the level's when the seed is 0).
- Not read: `EnderChestBlockEntity`, needed for the ender chest's openers' count, lid,
  sounds and `stillValid`.
- Not read: `WeatheringCopperChestBlock`, needed only for the copper chests' weathering
  by random ticks (their joining and sounds are read, 3.3).
- Not read: `ItemPredicate` (and its codec), needed for what a `lock` matches and the
  form of the `lock` key (**Inferred**: `NO_LOCK`, the empty predicate, passes any
  stack).
- Not read: `Shulker.getProgressAabb` and `getProgressDeltaAabb`, needed for the shulker
  box's shapes, the room its lid needs and the band whose entities it pushes.
- Not read: `LevelAccessor.gameEvent(Entity, Holder, BlockPos)` (the interface's
  default, which `Level` does not override), needed for the game event's position
  (**Inferred** the block's centre).
- Not read: `ClientboundBlockEventPacket`, needed to confirm the block event's field
  order (taken from the R-steps' encoder).
- Not read: `ItemStackWithSlot`, needed for the `Items` entries' exact NBT (the slot's
  key and type).
- Not read: `ExperimentalRedstoneUtils.initialOrientation`, needed for the orientation
  the trapped chest's updates carry (**Inferred**: none without the redstone
  experiment, as for every other block Clustine updates).
- Not read: a `javap -v` of `ChestBlock`, needed to confirm which lambda of `combine` is
  the "never blocked" test.
- Not read: `Cat.isInSittingPose`, needed (N7) for the cat's blocking.
### 12.3 Menus and clicks (3.10 to 3.21)

Read since the first draft: `ContainerInput`, `ClickAction`, `RemoteSlot` (and `$1`,
`$Synchronized`), `ServerPlayer$1`, `$2` and `$3`, `HashOps` (and its builders),
`ByteBufCodecs` and its nested codecs, the four packets, `InventoryMenu` (and `$1`),
`ArmorSlot`, `ResultSlot`, `ShulkerBoxSlot`, `CrafterSlot`, `NonInteractiveResultSlot`,
`Inventory`, `MenuProvider`, `SimpleMenuProvider`, `Attributes`. Still not read:

- Not read: `ServerPlayer$1$1` (the synchroniser's cache loader), needed for exactly how
  a component is hashed (**Inferred**: its value encoded with the type's codec through
  `HashOps.CRC32C_INSTANCE`, then `asInt()`), and a `javap -v` of `ServerPlayer$1` and
  `HashOps`, needed for the method references (`cache::getUnchecked`, the
  `HASH_COMPARATOR`'s `HashCode::padToLong`, **Inferred**).
- Not read (a library, not the game): Guava's `Crc32cHashFunction`, `AbstractHasher` and
  `HashCode`, needed for the byte order of the hasher's `putShort`/`putInt`/`putLong`/
  `putFloat`/`putDouble`/`putUnencodedChars` and of `HashCode.asBytes` (**Inferred**
  little-endian, as Guava documents); a test against a real client settles it.
- Not read: `FriendlyByteBuf` (`readContainerId`/`writeContainerId`), needed to confirm
  that the container id is a VarInt (**Inferred**).
- Not read: `ByIdMap` (`continuous` with `OutOfBoundsStrategy.ZERO`), needed to confirm
  that a click kind outside 0–6 decodes as `PICKUP` (**Inferred**).
- Not read: `TransientCraftingContainer`, needed for the 2×2 grid's notifying its menu
  (`slotsChanged`) and so when its result is recomputed.
- Not read: `BundleItem`, needed for bundles in menus (`overrideStackedOnOther`,
  `overrideOtherStackedOnMe`, `canFitInsideContainerItems`); not N6's.
- Not read: `MenuConstructor` and the blocks' `getMenuProvider` lambdas, needed only for
  which constructor a `SimpleMenuProvider` calls (3.1–3.9 name the menus from the
  blocks).
- Not read: `PacketUtils.ensureRunningOnSameThread` and the handling of a
  `ReportedException` from a packet, needed only to say what the game does with a
  malformed `SWAP` or drag (the port refuses it either way); and `MinecraftServer`,
  needed for where packet handling falls against the level's tick (the order of
  `open_screen` and the lid's block event, 3.10).
### 12.4 Dispensers, droppers and the crafter (sections 4 and 7)

Read in the second dump and settled here: `RandomizableContainer`,
`Container.getMaxStackSize`, `RandomSource.triangle`, `GameRules` (`tnt_explodes` true),
`BucketItem`, `BucketPickup`, `BoneMealItem`, `BlockItem.place`, `DirectionalPlaceContext`,
`Direction`, `ProjectileItem` and `DispenseConfig`, `CrafterSlot`,
`NonInteractiveResultSlot`, `ResultContainer`, `ItemStack.hurtAndBreak`,
`Entity.shearOffAllLeashConnections`, `Level.playSound`/`ServerLevel.playSeededSound` and
`addFreshEntity`, `FireBlock.onPlace`, `EntitySelector.CONTAINER_ENTITY_SELECTOR` (by its
lambda's order), `ServerboundContainerSlotStateChangedPacket`, the default
`BlockBehaviour.canBeReplaced(state, fluid)`. Still unread:

- Not read: `LootTable.fill` (with `getAvailableSlots`, `shuffleAndSplitItems`), needed
  for how a dispenser's structure loot is spread and what a seed of 0 draws from (4.1).
- Not read: `BucketPickup.pickupBlock` of `LiquidBlock`, `SimpleWaterloggedBlock` and
  `PowderSnowBlock`, and `LiquidBlock.canBeReplaced(state, fluid)` if it overrides the
  default, needed for what a dispenser's empty bucket takes and where a full one may
  pour (4.5).
- Not read: `BaseFireBlock` (`canBePlacedAt`, `getState`, `onPlace`), `CampfireBlock`,
  `CandleBlock` and `CandleCakeBlock` (`canLight`), needed for flint and steel (4.11).
- Not read: the `BonemealableBlock`s (`isValidBonemealTarget`, `isBonemealSuccess`,
  `performBonemeal` of crops, saplings, grass, flowers, seagrass, …), needed for bone
  meal's growth and its draws (4.12).
- Not read: `EnchantmentHelper.processDurabilityChange`, needed for Unbreaking's draw
  on an enchanted flint and steel or shears (4.11, 4.16).
- Not read: `WitherSkullBlock.canSpawnMob`/`checkSpawn`, `CarvedPumpkinBlock.
  canSpawnGolem`, `RotationSegment.convertToSegment`, needed for the skull's rotation and
  the golem and wither patterns (4.13).
- Not read: `HoneycombItem.getWaxed`, `RespawnAnchorBlock.charge`, `BeehiveBlock.
  releaseBeesAndResetHoneyLevel`/`dropHoneycomb`, needed for honeycomb, glowstone and
  bottles on hives (4.14).
- Not read: `ProjectileItem$DispenseConfig$Builder` (the defaults), each projectile
  item's `createDispenseConfig` and `asProjectile`, and `Projectile.
  spawnProjectileUsingShoot`/`shoot`, needed for N9's dispensed projectiles (4.17).
- Not read: `SulfurCube` (`equipItem`, `primeTime`) and `SpawnEggItem`, needed for N7's
  cubes and eggs from dispensers (4.10, 4.16).
- Not read: `RecipeCache`, `CraftingInput`, `RecipeManager`, the crafting recipe classes
  (`ShapedRecipe`, `ShapelessRecipe`, `TransmuteRecipe` and the special ones),
  `CraftingRecipe.getRemainingItems`, `ItemStack.onCraftedBySystem`, needed for the
  crafter's craft (7.2).
- Not read: `FriendlyByteBuf.writeContainerId`/`readContainerId`, needed for the exact
  bytes of the container id in `container_slot_state_changed` (as in every menu packet,
  12.3).
- Not read: `LevelEvent`, needed only to confirm the level events' names (quoted from
  SteelMC's extraction; the numbers are the bytecode's).
- Not read: a `javap -v` of `DispenseItemBehavior` and `EntitySelector`, needed to tie
  the lambdas to their fields for certain (`NOOP`, `CONTAINER_ENTITY_SELECTOR`).

### 12.5 Hoppers, plates and tripwire (sections 5 and 6)

- Not read: `AbstractFurnaceBlockEntity`, `BrewingStandBlockEntity`,
  `ChiseledBookShelfBlockEntity`, `DecoratedPotBlockEntity`, `JukeboxBlockEntity`,
  `ShelfBlockEntity`, needed for which other block entities are containers to a hopper
  and their faces' slots and `canPlaceItem` rules (a creative player meets them all).
- Not read: `EntitySection` (and `ClassInstanceMultiMap`), `LevelEntityGetterAdapter`
  and `SectionPos.asLong`, needed for the order of entities within a section, the
  per-entity box test (**Inferred** `AABB.intersects`) and the exact order of sections
  within one x (5.6).
- Not read: `SynchedEntityData.set`, needed to confirm that a hopper's partial take of an
  item entity always marks its `DATA_ITEM` dirty.
- Not read: `LegacyRandomSource.nextInt`, needed only to know that `nextInt(1)` draws
  (the oracle cannot compare the level's random anyway).
- Not read: `LootTable.fill` and `LootContext$Builder.create`, needed for how a loot
  chest's loot is spread over slots (section 3) and which random the composter's chance
  draws from.
- Not read: a `javap -v` of `HopperBlock` and `EntitySelector`, needed to confirm the
  ticker's method reference and which lambda is `CONTAINER_ENTITY_SELECTOR`.
- Not read: `SoundEvents`, needed to confirm the plates' sound fields' ids (all twelve
  are in the registry by their usual names).
### 12.6 Sculk sensors and vibrations (section 8)

Read since the first version, and settled there: `GameEvent` (each event's radius),
`GameEventDispatcher`, `GameEventListener` (`UNSPECIFIED` by default), `LevelChunk`
(when listeners are registered), `VibrationSelector`, `VibrationInfo`,
`CalibratedSculkSensorBlockEntity` and its user, `ItemEntity` (`hit_ground`, wool),
`FallingBlockEntity`, `BlockGetter.isBlockInLine`, `Block.playerWillDestroy`,
`BlockItem.place`, `BucketItem`. Still not read:

- Not read: `EuclideanGameEventListenerRegistry` (and `GameEventListenerRegistry`),
  needed for the order of listeners within a section and the range test
  (**Inferred** `distSqr` of block positions ≤ radius², in the order registered).
- Not read: `BlockPositionSource`, `EntityPositionSource`, `PositionSource(Type)` and
  `VibrationParticleOption`, needed for the listener's position (**Inferred** the
  block's centre) and the particle's wire format.
- Not read: `ClientboundLevelParticlesPacket` (and its `RandomizationType`), needed for
  the particle packet's fields in 26.3.
- Not read: `LevelAccessor` (its `gameEvent` defaults), needed for the position a
  block-position post uses (**Inferred** `Vec3.atCenterOf`) and the `ResourceKey`
  overload used by resonance.
- Not read: `RegistryCodecs.holder`, needed to confirm that a saved vibration names its
  event by registry name.
- Not read: `LeverBlock`, `ButtonBlock`, `DoorBlock`, `TrapDoorBlock`,
  `FenceGateBlock`, `NoteBlock`, `PistonBaseBlock`, `BellBlock`, `RepeaterBlock`,
  `FlintAndSteelItem`: needed for the game events the blocks and items already built
  post (**Inferred** in 8.0 item 2 and 8.1).
- Not read: `Entity.playStepSound`/`walkingStepSound`'s sound-type lookup, needed only
  if step sounds are sent as well.
- Not read: `SculkShriekerBlock`, `SculkShriekerBlockEntity`, `Warden`, `Allay`: the
  other vibration listeners, needed only when they are built (after N6).

