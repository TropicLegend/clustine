# W6: placing, using and where a new player stands

The plan for step W6 of `terrain-plan.md`. It was written on 2026-10-14, reviewed
independently the same day, and revised by that review (section 8) and by the owner's
answers to the questions the review raised. It says what a player meets after W6,
what is built where, in which order, who writes what and how each part is judged.

## 1. What W6 was, and what is left of it

The plan's row: "`--seed`; version in the settings; spawn x, z from the generator,
fixed for a world, and a joining player put on the highest block of that column as it
is now; placing into grass, water, snow replaces; a click on a block that is used when
clicked places nothing; doors, trapdoors and fence gates open and close."

- `--seed` and the generator's version in the settings came with W5 and are done.
- Spawn x, z come from the generator (`TerrainGenerator::spawn_point`, the block at the
  origin for now). **Fixing them for a world moves to T8**, which is where the spawn
  search of the official server arrives: until then every generator says the origin,
  and a spawn written into a world now could only ever say the origin too. T8 records
  the spawn in the world's `meta` when the world is made, the store reads it from
  there, and the coordinator takes it from the store with the list of regions
  (`StoreHello::Regions`), rather than from its own `--seed`. Written into the T8 row.
- **What the owner chose on 2026-10-14**, beyond the plan's row:
  1. **Placement by vanilla's common rules**: a placed block takes the state vanilla
     gives it, for the classes of section 4.
  2. **Shapes from the neighbours when placed**: a fence, pane, wall, iron bars,
     stairs or redstone wire takes the shape its neighbours give it as it is placed.
     The neighbour does not change in turn until phase B's neighbour updates (B1), so
     a fence placed beside a fence connects towards it and the old one does not
     connect back; a pane between two stone blocks is a window at once.
  3. **Buckets**: a water or lava bucket places a source, an empty bucket takes one,
     and a lily pad goes on water. The water does not flow until B5, as today.

## 2. Where a new player stands

**Now**: a player without a place, or with one below the world, enters at
`RegionConfig::spawn`, a fixed point whose height the generator said when the server
started. Once someone digs or builds there, a new player enters inside a block or in
the air.

**After W6**: the column stays `spawn.x`, `spawn.z`. The height is found when the
player is placed (in `Region::apply_entered`, where `at_spawn` is made today), from the
chunk as the region has it then:

- `top` is `min_y + motion_blocking`, the chunk's `motion_blocking` heightmap at the
  column (counted from the bottom of the chunk, 0 meaning an empty column:
  `chunk.rs:83-96`): the first block above the highest block that stops movement or
  holds a fluid.
- The player's feet are at `top - 1` plus the highest `max_y` of the collision boxes
  of the block at `top - 1`, or at `top` if it has none (water). So a player stands on
  a fence (1.5 high) or a wall rather than inside it, on a bottom slab or a carpet at
  its top, on the water over the sea floor, and on a tower built at the spawn.
- If the column is empty (`motion_blocking` 0), as `spawn.y` says.

The region can read the column only when it holds and has loaded the home chunk:

- **The home region keeps the home chunk loaded.** It is never given back
  (`Region::settle_chunks`), but today it is loaded only while it has a ticket. In
  `update_chunks` the home chunk counts as wanted whenever it is held: asked of
  storage like a chunk with a ticket, kept in `chunks` when its last ticket goes,
  and taken when it arrives without one. This changes what the code and the tests
  hold; W6e changes each, with the reason:
  - the invariant check "a loaded chunk is held and has a ticket" (`region.rs`, near
    1995) gains "or is the home chunk";
  - the filter in `chunks_loaded` (`self.tickets.contains_key(position)`, near 1446)
    lets the home chunk in;
  - worker tests that use the origin as their chunk and wait for it to unload
    (`changes_survive_a_chunk_being_unloaded`,
    `a_chunk_is_kept_until_its_last_subscriber_lets_go_and_is_saved_then` in
    `services/worker/src/lib.rs`) move to a chunk that is not the home chunk;
  - counts of loaded chunks that expect zero (`services/worker/tests/reshape.rs`,
    near 2874 and 4302) count the home chunk.
- **When the home chunk is not loaded**, the player enters at `spawn.y` as today and
  is not moved when the chunk arrives. That happens in the first ticks after **every
  restore of the home region**: a start, a take-over, a merge and a split all begin
  with no chunk loaded, and entering stays are answered in exactly those ticks. After
  a merge or a split the worker's warm cache makes it about one tick; after a
  take-over the chunk's read and the store's answers race. It also happens to a join
  that reached a region not holding the spawn (a routing table out of date). Named in
  the known limits, as "a new player can enter at the generator's height in the first
  ticks after the home region was started, taken over, merged or split".
- **Nothing of the hand-over changes**: a player handed on carries a pose as today. A
  place that a store kept is used as it is, also on a block someone has since built
  there (as in the official server, which does not move a player who logs in inside a
  block; the client pushes them out).
- **Determinism**: the height is a function of the region's chunk.

## 3. A click, from the client to the region

### 3.1 What the edge passes on

`PlayerInput::UseItemOn` gains what vanilla decides by:

- `cursor: [f32; 3]`, where on the clicked block the click landed (the protocol's
  field, relative to the block; the edge clamps each to 0..1, and a value that is not
  finite counts as 0.5);
- `sneaking: bool`, whether the player held the sneak key when they clicked.

`PlayerInput::UseItem` is new, for a click that hits no block or one the held item
does not use on a block (buckets, lily pads): `sequence`, `sneaking` as for
`UseItemOn`, and the rotation the packet carries. `ServerboundUseItem` has the
player's yaw and pitch, and the region makes them the pose's rotation before it uses
the item, as vanilla's `absSnapRotationTo` does.

The edge learns the sneak key from `ServerboundPlayerInput` (`minecraft:player_input`,
its flags byte, `0x20` being shift), which it now decodes. It keeps the last flags in
the connection's loop and puts `sneaking` into each `UseItemOn`. The client sends its
input packet before a click that follows a change of keys, on the same connection, so
the order is the client's. Nothing of it is region state.

The player's facing comes from the region's pose (`Pose`'s `yaw` and `pitch`), the
rotation of the last movement packet, as vanilla's is.

Only the main hand holds anything. A click with the off hand is acknowledged and does
nothing (as today).

### 3.2 The order of a click (vanilla's `ServerPlayerGameMode.useItemOn`)

For a click on block `P` with face `F`, by a player within reach. A click with an
empty hand goes on too: **the early return when no block is held
(`region.rs:1205-1210`) goes**, and such a click is sent to the region that holds `P`
like any other, since it may open a door.

1. **Use.** Unless the player sneaks and holds something, the clicked block is used if
   its block is `used_by_click` (`BlockInfo::used_by_click`, from the jar). A use
   consumes the click: nothing is placed, also when what is used does nothing yet.
   What a use does in W6:
   - **doors** (`DoorBlock` whose material opens by hand, which is what
     `used_by_click` says): `open` flips on the clicked half, and on the other half if
     the block at the other half's place is the same door with the other `half`;
   - **trapdoors** (`TrapDoorBlock`, likewise): `open` flips;
   - **fence gates** (`FenceGateBlock`): if open, `open` becomes false; if closed,
     `open` becomes true and, if the gate's `facing` is the opposite of the player's
     horizontal facing, `facing` becomes the player's facing;
   - **flower pots** (`FlowerPotBlock`): an empty pot clicked with an item whose
     `Item::places` is the block some pot holds (`BlockInfo::potted`, from each pot's
     `potted` field in the jar, which is what vanilla's `POTTED_BY_CONTENT` is made
     of) becomes that pot: the azalea goes into `potted_azalea_bush`, the bamboo item
     (which places `bamboo_sapling`) into `potted_bamboo`. A potted pot clicked with
     anything does nothing (taking the plant out gives an item, and there is no
     inventory beyond the hotbar);
   - **every other used block** does nothing, named: chests, crafting tables,
     furnaces, beds, buttons, levers, note blocks, cakes and the rest, and the blocks
     that vanilla uses only in some states and places against in others (redstone
     wire that is neither a dot nor a cross, a young sweet berry bush, cave vines
     without berries, an uncharged respawn anchor, a note block clicked on top with a
     head in hand).
2. **Place.** Otherwise, if the held item places a block (section 3.3) and is not used
   only on fluids, it is placed by section 4. Otherwise nothing happens here; the
   client then sends `UseItem` (section 5).

The click is acknowledged in every case (`player.acknowledge(sequence)`), after its
changes, as today.

### 3.3 What an item places

`Item::block` (the block of the same name) is not enough. Datagen now gives, for each
item, from the jar's item registry (`tools/datagen/java/Extract.java`, `items()`):

- `Item::class`, the item's Java class (`ItemClass`);
- `Item::places`, the id of the block it places (`BlockItem.getBlock()`), if any;
- `Item::wall`, for a `StandingAndWallBlockItem` (torches, signs, hanging signs,
  banners, heads, coral fans), the wall block and the attachment direction.

How an item places is decided by these, not by class names of items that vary:

- `wall` is set: the standing-or-wall rule (section 4.4).
- `class` is `DoubleHighBlockItem`: the block and the one above it (doors, tall
  plants).
- `class` is `PlaceOnWaterBlockItem` (lily pad, frogspawn): nothing from `UseItemOn`;
  it places only through `UseItem` (section 5).
- `class` is `GameMasterBlockItem` (command blocks, structure blocks, jigsaws, test
  blocks): nothing; only an operator places one, and Clustine has none.
- `class` is `ScaffoldingBlockItem`: placed as a plain block; extending scaffolding
  sideways when it is clicked is named.
- otherwise, if `places` is set: the block's own rule (section 4), chosen by the
  **block's** class: a bed is a `BlockItem` placing a `BedBlock`, which places its
  head itself.

**Classes are matched with their ancestry.** `BlockInfo::class` is the block's own
class; 26.3 has many classes made from another (`WeatheringCopperDoorBlock` from
`DoorBlock`, `RepeaterBlock` and `ComparatorBlock` from `DiodeBlock`, `FurnaceBlock`
from `AbstractFurnaceBlock`, `StainedGlassPaneBlock` from `IronBarsBlock`). Datagen
gives each class the chain of classes it is made from (`BlockClass::ancestors`), and
a class named in a rule matches every block whose class is it or is made from it
(`BlockInfo::is`). Where two rules could match, the one for the nearer class wins.
W6c has a test that every block whose class is named in 4.2, 4.3 or 4.4, or made
from one, finds the rule meant for it.

## 4. Placing

SteelMC's branch `26.3` at commit `885c4b3` (`~/reference/SteelMC-26.3`, the commit
`NOTICE.md` cites) is the reference for vanilla's rules where it has them; the builder
attributes what is adapted from it in `NOTICE.md`. Where SteelMC lacks a class, this
section states the rule, and the comparisons with the official server (W6f) judge
every rule in the end.

### 4.1 The context

As vanilla's `BlockPlaceContext`:

- **Replacing the clicked block** (`replaceClicked`): if the block at `P` can be
  replaced by this context (4.2), the target `T` is `P` itself; otherwise `T` is `P`'s
  neighbour across `F`, and the block at `T` must be replaceable by this context, or
  nothing is placed. A click on air (a block the client still showed) is a click on a
  replaceable block, so the block is placed at `P`, as in vanilla.
- **Looking directions.** `orderedByNearest` is vanilla's `Direction.orderedByNearest`
  of the player's yaw and pitch, computed with vanilla's sine table (the `Mth.sin`
  table `clustine-noise` already has), not with `f32::sin`: the two differ at 45° and
  between platforms. The **nearest looking directions** of a context are that order
  with the opposite of `F` moved to the front unless `replaceClicked`. The **nearest
  looking direction** is `orderedByNearest[0]`, without that move. The **horizontal
  direction** is the player's facing from the yaw alone (`Direction.fromYRot`). Each
  rule below says which it uses.
- **Within the world**: `T.y` within the world's height, and for two-block states the
  other block too.
- **The click location**: `P` plus the cursor; "the cursor's height above `T`" is that
  location's y less `T.y` (which differs from the cursor's y when `T` is not `P`).

### 4.2 Can be replaced

`canBeReplaced(state, context)`:

- by default, the state's `can_be_replaced` (from the jar: air, water, lava, short
  grass, fern, dead bush, vines and the rest of `#replaceable`) and the held item is
  not the item of that block;
- `SnowLayerBlock`: with snow in hand, if `layers < 8` and (not `replaceClicked`, or
  `F` is up); with anything else, if `layers` is 1;
- `SlabBlock`: a single slab, with a slab of the same block in hand: if not
  `replaceClicked`, always; if clicked, a bottom slab when `F` is up or the cursor's
  height above `T` is above 0.5 on a side, and a top slab when `F` is down or that
  height is below 0.5 on a side;
- `CandleBlock`, `SeaPickleBlock`, `TurtleEggBlock`, `FlowerBedBlock` and
  `LeafLitterBlock`: a block of the same kind in hand, **while not sneaking**, adds one
  more where the block holds fewer than its most;
- `MultifaceBlock` (glow lichen, sculk vein and the rest made from it): replaceable
  unless the item in hand is its own, or while a face is still free; the state placed
  adds a face along the nearest looking directions, as vanilla's
  `MultifaceBlock.getStateForPlacement`;
- fluids: replaceable.

### 4.3 Survival, obstruction and all-or-nothing

- **Survival**: a state is placed only if it can survive at `T` by its class's
  `canSurvive`. `BlockInfo::survival_rule` says, from the jar, whether a block's class
  or one it is made from has a `canSurvive` of its own. W6 ports `canSurvive` for
  every class that 4.2, 4.4 and 3.3 give a rule and that has one, written out here:
  `BushBlock` and the plants made from it, saplings, flowers, `TallFlowerBlock` and
  `DoublePlantBlock` (the `#supports_*` tags of 26.3: `#supports_vegetation` and the
  others, not "`#dirt` or farmland"), `CropBlock` and `StemBlock` (`#supports_crops`
  below; the light they need is not judged, named), `SugarCaneBlock` (with water
  beside its ground), `CactusBlock`, `CarpetBlock` (anything but air below),
  `BaseRailBlock`, `BasePressurePlateBlock` (a sturdy or rigid top below),
  `SnowLayerBlock` (a full top face below, or 8 layers of snow, soul sand or honey,
  and not ice or packed ice), `DoorBlock` (a sturdy top below), `DiodeBlock` (a rigid
  top below), `BaseTorchBlock` and the wall torches, `StandingSignBlock`,
  `WallSignBlock`, `CeilingHangingSignBlock` (the block above holds it at its
  centre), `WallHangingSignBlock`, `BannerBlock`, `WallBannerBlock`,
  `FaceAttachedHorizontalDirectionalBlock` (buttons and levers; the grindstone's is
  always true), `LadderBlock`, `LanternBlock`, `RedStoneWireBlock`, `TripWireBlock`,
  `WaterlilyBlock`, `CandleBlock`, `SeaPickleBlock`, `TurtleEggBlock`,
  `MultifaceBlock`, `FlowerBedBlock`, `LeafLitterBlock`, `SmallDripleafBlock`,
  `BigDripleafBlock`, `ScaffoldingBlock`, the coral fans (`BaseCoralFanBlock` and its
  wall form). W6c checks this list against `survival_rule`: **a block whose class has
  a `canSurvive` and is not in the list places nothing**, named, rather than placing
  where vanilla would refuse.
- **Nobody in the way**: as vanilla, the placed state's collision boxes
  (`collision_shape`, from the jar) at `T` may not overlap a player's box (0.6 wide;
  1.8 high, 1.5 while sneaking for the placer). Only the block at `T` is checked; the
  upper half of a door and a bed's head are set without the check, as in vanilla. The
  players checked are those the region has; a player of another region whose box
  crosses into `T`'s chunk is not (as today), named.
- **All or nothing**: a placement or a use gives a list of changes. It is applied only
  if every position in it is in a chunk the region holds and has loaded; otherwise
  nothing changes. `T`, or `P` when the step does not carry it, in a chunk the region
  has not loaded means nothing is placed, never that the block there is read as air.
  If the region has `P` loaded (a reshape gave it the chunk since), what it has wins
  over the state the step carries.

### 4.4 The rules, by the class of the block placed

Every rule adds `waterlogged = true` for a block that has the property when `T` holds a
water source (a double slab excepted), and `false` otherwise.

| Class | State |
|---|---|
| `RotatedPillarBlock` (logs, wood, stems, basalt, pillars, hay, bone, deepslate), `ChainBlock` | `axis` = the axis of `F` |
| Facing the opposite of the horizontal direction: `AbstractFurnaceBlock` (`FurnaceBlock`, `SmokerBlock`, `BlastFurnaceBlock`), `CarvedPumpkinBlock`, `BeehiveBlock`, `LoomBlock`, `StonecutterBlock`, `LecternBlock`, `GlazedTerracottaBlock`, `EnderChestBlock`, `ChiseledBookShelfBlock`, `EndPortalFrameBlock`, `DiodeBlock` (`RepeaterBlock`, `ComparatorBlock`), `SmallDripleafBlock`, `BigDripleafBlock` | `facing` |
| `ChestBlock` and the classes made from it (trapped and copper chests) | `facing` as above, `type` single: a double chest needs the other chest to change, which is B1's |
| `CampfireBlock` | `facing` = the horizontal direction (not its opposite); `lit` = not waterlogged; `signal_fire` if hay is below |
| `AnvilBlock` | `facing` = the horizontal direction turned clockwise |
| `BarrelBlock`, `DispenserBlock`, `DropperBlock`, `PistonBaseBlock` | `facing` = the opposite of the nearest looking direction |
| `ObserverBlock` | `facing` = the nearest looking direction |
| `EndRodBlock` | `facing` = `F`, or its opposite if the block at `T` moved against `F` (usually `P`) is an end rod facing `F` |
| `LightningRodBlock` | `facing` = `F` |
| `StairBlock` | `facing` = the horizontal direction; `half` top if `F` is down, or `F` is not up and the cursor's height above `T` is above 0.5; `shape` from the stairs beside it as vanilla's `getStairsShape` (owner's choice 2) |
| `SlabBlock` | `type` double where it joins a slab (4.2), not waterlogged; else top if `F` is down or (`F` is not up and the cursor's height above `T` is above 0.5), else bottom |
| `DoorBlock` | `facing` = the horizontal direction; `hinge` by vanilla's `getHinge` (the two blocks to the left and the right of `T` and above them, whether each has a full collision block, whether the block beside is a door's lower half, then the click location); `half` lower at `T` and upper above; `open` and `powered` false (redstone at placement is named) |
| `TrapDoorBlock` | if not `replaceClicked` and `F` is horizontal: `facing` = `F`, `half` top if the cursor's height above `T` is above 0.5; otherwise `facing` = the opposite of the horizontal direction, `half` bottom if `F` is up, else top; `open` and `powered` false |
| `FenceGateBlock` | `facing` = the horizontal direction; `in_wall` if a wall is beside it across its axis |
| `FenceBlock`, `IronBarsBlock` (panes, stained panes, bars, copper bars), `WallBlock` | connections from the neighbours as placed, as vanilla's `getStateForPlacement`: a fence joins a fence of its kind (`#wooden_fences` together, nether brick only nether brick), a gate whose axis allows it, or a sturdy face that is not an exception (`Block.isExceptionForConnection`: leaves, barrier, carved and plain pumpkins, melon, shulker boxes); a pane joins panes, bars, walls and sturdy faces that are not exceptions; a wall's `up` and side heights from the block above; the neighbours do not change until B1 |
| `RedStoneWireBlock` | vanilla's connection state from the neighbours, a cross where nothing connects; the neighbours do not change until B1 |
| `BaseRailBlock` (rails, powered, detector, activator) | `shape` east_west or north_south by the horizontal direction; slopes and curves need B1 |
| `BedBlock` | `facing` = the horizontal direction; foot at `T`, head one block that way, which must be replaceable and within the world; no support is needed |
| standing-or-wall (`Item::wall` set) | vanilla's `StandingAndWallBlockItem.getPlacementState`: the wall block's own state for placement, computed once (its own loop over the horizontal nearest looking directions, the first that can survive); then for each of the nearest looking directions, skipping the opposite of the attachment direction: the attachment direction gives the standing block (or the hanging one), a horizontal direction gives that wall state; the first that can survive and is unobstructed. Standing signs and banners take `rotation` = (yaw + 180) in sixteenths; heads and skulls the yaw as W6f finds it |
| `FaceAttachedHorizontalDirectionalBlock` (`ButtonBlock`, `LeverBlock`, `GrindstoneBlock`) | for each of the nearest looking directions: up or down gives `face` ceiling or floor with `facing` = the horizontal direction, a side gives `face` wall facing away from it; the first that can survive |
| `LadderBlock` | if not `replaceClicked` and the clicked block is a ladder facing `F`, nothing; else for each of the nearest looking directions that is horizontal, `facing` = its opposite, the first that can survive |
| `LanternBlock` | for each of the nearest looking directions that is vertical, `hanging` if it is up, the first that can survive |
| `DoublePlantBlock` and `TallFlowerBlock` (tall grass, large fern, sunflower, lilac, rose bush, peony, pitcher plant) | lower half at `T`, upper above, which must be replaceable |
| `SnowLayerBlock` | `layers` one more where it joins snow (4.2), else 1 |
| `CeilingHangingSignBlock`, `WallHangingSignBlock` (through the standing-or-wall rule, attachment up) | under a block, the ceiling sign: `attached` if the block above has no full bottom face (`SupportType::Full`), or (vanilla, to be confirmed by W6f) while sneaking; when attached, `rotation` from the yaw in sixteenths, otherwise snapped to the four sides, and under another hanging sign along that sign's axis. On a side, the wall hanging sign |
| `CropBlock`, `StemBlock`, `SugarCaneBlock`, `CactusBlock`, plants, `ScaffoldingBlock` | the default state, with survival (4.3) |
| every other class | the block's default state |

### 4.5 Blocks a rule reads, across regions

A rule reads blocks other than `T`: below (survival), behind (attachment), beside (a
hinge, connections, a slab or snow to join, a wall beside a gate), and the clicked
block `P`.

- **The clicked block travels with the click.** When `T` is in another region's chunk
  than `P`, the step passed on carries `P`'s state as `P`'s region read it, and that
  `replaceClicked` is false. `T`'s region reads `P` from the step (unless it has `P`
  loaded itself, 4.3). A torch on the side of a block across a border goes on that
  block's side. `P` may have changed between the two regions' ticks (a wall broken in
  between): the torch is then placed against what was there. Named.
- **Any other block** of a chunk the region has not loaded is read as air: a hinge, a
  connection, a gate in a wall and a bed's head across a border may then differ from
  the official server's. Named. Pinned borders (`--pin`, the tests' `0,4`) are where
  this is met.

### 4.6 Across regions

Today a click on a block another region holds goes there as `RemoteStep::PlaceAgainst`
or `RemoteStep::Place`, carrying the block state and the placer's position. Two
**new variants are appended** to `RemoteStep` (old ones keep their indices, so what is
stored decodes as before and `STATE_FORMAT` stays):

- `UseOn { click }`: the whole click (position `P`, face, cursor, sneaking, the held
  item, the placer's pose), for the region that holds `P`. That region applies 3.2: a
  use, or a placement with its target; a target in yet another region is passed on
  as:
- `PlaceAt { click, target, clicked }`: the click, the target `T` and `P`'s state as
  read (4.5), for the region that holds `T`, which applies section 4 with
  `replaceClicked` false.

The old variants are still carried out as today when a restored state holds them
(place the given state into air), since a rolling update restores a region written by
the old build (`deploy/kubernetes/worker.yaml`). The test of a state's bytes
(`the_bytes_of_a_state_and_of_a_delta_are_as_written_down`) still passes. The links
between services have no version: during a rolling update an old worker cannot read a
new variant from a new one, as for any change of a message; named.

### 4.7 What tests say today that W6 changes

These were written from a specification by someone who did not write the code; each
change is a decision, made here:

- `a_placement_against_no_block_of_a_held_chunk_is_acknowledged_and_goes_nowhere` and
  `a_remote_placement_against_no_block_of_a_held_chunk_is_done_and_goes_nowhere`
  (`crates/clustine-sim/tests/chunks.rs`): a click on air places at `P` (4.1), as the
  official server does. They are changed to say so.
- `a_placement_without_a_block_in_hand_or_out_of_reach_is_acknowledged_whatever_the_chunks`:
  a click with an empty hand on another region's block is now sent there (3.2); out of
  reach still sends nothing.
- `bin/clustine/tests/terrain.rs` (near line 241) asserts that a new player enters at
  `spawn_point()`. That holds while the home column is as generated; it is changed to
  say "on the column's top as the chunk has it", which stays true once trees are
  generated.

## 5. Buckets and lily pads (`UseItem`)

A `UseItem` with a water bucket, a lava bucket, an empty bucket or a lily pad (and
frogspawn) in hand, as vanilla's `BucketItem.use` and `PlaceOnWaterBlockItem.use`
(SteelMC 26.3, `items/bucket.rs`):

- **The rotation** the packet carries becomes the pose's first (3.1).
- **The ray**: from the player's eyes (feet + 1.62, 1.27 while sneaking) along the
  view vector, computed with vanilla's sine table, 5 blocks (the creative block
  reach), through blocks by their **outline shapes** (`StateProps::outline_shape`,
  which datagen now gives) and, for an empty bucket and a lily pad, through fluids: a
  source stops it. Plants whose outline the game shifts by position are hit as if
  unshifted, and a light block's outline does not depend on what is held; both
  named. The ray stops at the first block of a chunk the region has not loaded, and
  nothing happens then (named: at a region's border a bucket does nothing beyond
  it).
- **An empty bucket** at a source of water or lava, or at powder snow: the block
  becomes air, or a waterlogged block becomes not waterlogged. In creative the bucket
  in hand stays empty, and the filled bucket is put into the first empty hotbar slot
  if no hotbar slot holds one already (vanilla puts it anywhere in the inventory,
  which Clustine does not keep beyond the hotbar; named).
- **A water or lava bucket**, with the block hit `H` and the face hit `G`: the first
  target is `H` only for a water bucket on a block that can hold water (it has
  `waterlogged`), and only when not sneaking; otherwise it is the block across `G`,
  and if that one cannot take the fluid, the same block is tried again as vanilla's
  fallback does (which comes to nothing). A block can take the fluid if it is air,
  `can_be_replaced`, or not solid (`StateProps::is_solid`): torches, flowers and rails
  are washed away. While sneaking the first target must be air. A block that holds
  water is waterlogged rather than replaced. In creative the bucket stays as it is.
  Water placed where it evaporates (the Nether) is not a case yet; a bucket into a
  cauldron does not fill it (named).
- **A lily pad**: placed in the block above whatever the ray hit, if it can survive
  there (water, or ice, below) and nothing is in its way (it has collision).
- All or nothing, in the region's own chunks, as 4.3. A target in another region's
  chunk: nothing, named.

## 6. Order of building

| # | What | Who | Judged by |
|---|---|---|---|
| W6a | Datagen: item classes, placed blocks, wall blocks; outline shapes per state (layout 2 of `block_states.bin`, shapes shared with collision); whether a block's class has a `canSurvive`; the classes each class is made from; what each flower pot holds | me (needs the jar) | `datagen --check`; values for a dozen items, classes, pots and shapes |
| W6b | Protocol and edge: `ServerboundPlayerInput` (done in the tree) and `ServerboundUseItem`; `UseItemOn` carries `cursor` and `sneaking`; `UseItem` is passed on; the bots can send all three | me | Codec tests; the official server takes the bots' packets (W6f) |
| W6c | `crates/clustine-sim/src/placement.rs` (and modules beside it as it grows): pure functions over a context and a closure that reads a block (`None` for a block not loaded): `click(context, read) -> Outcome` (a use, a placement as a list of changes, nothing) and `use_item(context, read) -> Outcome` (section 5). No region state | a subagent in a worktree, with sections 3 to 5 as its brief and `~/reference/SteelMC-26.3` as its reference, attributing what it adapts in `NOTICE.md` | Its own unit tests |
| W6d | Tests of W6c from this document by someone who did not write it: the order of a click, the context (replacing, looking directions, the cursor's height above `T`), 4.2, 4.3, the all-or-nothing rule, the rows of 4.4 that this document states fully, section 5. The rows that say "as vanilla" or "as SteelMC" are W6f's to judge | another subagent | The tests pass against W6c, and what they find is fixed |
| W6e | The region: clicks and `UseItem` through `placement`; the new `RemoteStep`s; the home chunk kept loaded and the spawn height (section 2); the tests of 2 and 4.7 | me (the steps across regions are hand-over logic) | Simulation tests; the existing tests of placing across regions |
| W6f | Comparisons with the official server: for each block item class of section 4.4 and a set of clicks (faces, cursors, yaws and pitches at and near 45°, sneaking, onto grass, into water, onto snow, against a door, against a wall across a pinned border), the state Clustine places equals the official server's; a door, trapdoor and gate opened and closed; a bucket emptied and filled; a new player on a tower and on a fence at the spawn | written by a subagent, run by me | Ignored tests, run on this machine |

W6a and W6b first, as the shared contracts (pushed before W6c starts, so its worktree
has them). W6c and W6d at the same time. W6e after W6c. W6f after W6e.

## 7. What a player meets after W6, and what is named

- A new player stands on whatever is at the spawn column, also on a fence or a slab.
- Clicking a door, trapdoor or fence gate opens and closes it; sneaking with a block in
  hand places against it instead. A flower goes into a pot. Clicking a chest,
  crafting table, furnace, bed, button or lever does nothing, and places nothing.
- Logs lie along the clicked face; stairs and slabs go where the cursor says, and
  stairs make corners with the stairs already there; furnaces and chests face the
  player; torches, signs, banners, buttons and ladders go on the wall clicked; doors
  and beds are two blocks with the right hinge; a slab on a slab makes a double slab;
  snow on snow grows; fences, panes and walls join what is beside them as placed;
  lanterns hang under blocks; rails lie along the player's facing. Placing into
  grass, water and snow replaces them; a block that can hold water does. Crops,
  flowers and sugar cane go only where vanilla lets them.
- Buckets place and take water and lava, which do not flow yet.

Named in the known limits after W6: a neighbour does not change in turn (fences and
panes connect one way, chests stay single, rails do not curve or slope) until B1;
breaking one half of a door, bed or tall plant leaves the other (B2); a placed sign
does not open its editor and keeps no text; the used blocks that do nothing, and the
blocks vanilla uses in some states only; redstone does not open a door or trapdoor
placed beside it; crops and mushrooms are placed whatever the light; the classes with
a `canSurvive` W6 does not port place nothing; scaffolding does not extend sideways;
blocks of a chunk a region has not loaded read as air for hinges, connections and a
bed's head (4.5); a bucket does nothing across a region's border; a player of another
region does not block a placement; a block clicked across a border may have changed
before the placement reaches it; a ray ignores a plant's offset and a light block's
held-item outline; a bucket does not fill a cauldron, and the filled bucket goes only
into the hotbar; flint and steel, axes, hoes, bone meal, spawn eggs
and the rest of the items that are not blocks do nothing; a new player can enter at
the generator's height in the first ticks after the home region was started, taken
over, merged or split (section 2); an old worker cannot read a new step during a
rolling update.

## 8. The review and what changed

An independent review on 2026-10-14 answered the plan's questions and found:

- **Defects, all taken**: the clicked block read as air across a border (now carried
  with the step, 4.5); beds needing support (they do not); placing without survival
  where vanilla refuses, made worse by W6a's new `places` (crops on stone, lily pads
  on land: now refused, 4.3, 3.3); vanilla's standing-or-wall algorithm and the two
  lists of looking directions (4.1, 4.4); the campfire's facing, lanterns, rails and
  the full list of facing classes; the rules that turn on `replaceClicked` for
  trapdoors, snow and slabs; old `RemoteStep`s must stay because of rolling updates
  (4.6); tests and invariants the plan changed without saying (4.7, section 2); a
  click on air and an empty-handed click.
- **Gaps, decided**: rules written as "as vanilla" are judged by W6f, not by W6d;
  buckets and connections at placement (owner, section 1); blocks used only in some
  states named, potting done; redstone at placement named; the spawn height on a
  fence.
- **Minor, taken**: open trapdoors have collision; only `T` is checked for players;
  the sneaking box; the sine table; dispatch on `Item::wall` and the block class; the
  ladder against a ladder; snow's survival; a door's other half only if it matches.
- The reference is SteelMC's branch `26.3` at `885c4b3`, which `NOTICE.md` cites, now
  checked out beside its `master`.

A second review of the revision found, and the plan now has: classes matched with
their ancestry, since 26.3 names most blocks by classes made from the ones the rules
name (3.3, with datagen's `BlockClass::ancestors`); the survival port list written
out by class (4.3), without which several of the plan's own rows would have placed
nothing; `P` carried by a step not being refused as "not loaded" (4.3); the bucket's
targets as vanilla has them, and sneaking with `UseItem` (section 5); potting by the
pots' contents from the jar, and the hanging sign's `attached` (3.2, 4.4); the
sneaking condition of the additive blocks and the multiface rule (4.2); the
exceptions to connections (4.4); the creative empty bucket, powder snow, the ray's
limits, the lily pad's place (section 5); the lightning rod without a flip; the
spawn height on a bottom slab (section 2); one more test that changes (4.7). Whether
a head that is obstructed in its first standing-or-wall choice tries the next is
left to W6f.
