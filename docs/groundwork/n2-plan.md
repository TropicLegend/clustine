# Step N2: entities in their chunks, durable, and the item at rest

Draft of 2026-10-18, revised the same day after an independent review (section 11). Step N2 of
[ADR-0028](../adr/0028-entities.md) (sections 2 and 3, and section 6 for entities other
than players), and the shared types its parts are built against. N1 (keys) is done.

## 1. What a player meets after N2

- **Q** throws the held item (one of it; ctrl+Q the stack), and a creative player
  dropping a stack out of the inventory screen throws it too (`SetCreativeModeSlot`
  with slot -1). The item appears in front of them, as the game's does, and lies there.
  It does not fall, float or get picked up until N4 (physics, pick-up), but it survives
  a crash, a restart, a merge, a split and a lend, and every client in view sees it.
- The **whole inventory** (36 slots, the off hand, armour) is kept, so that what a
  creative player puts in their inventory is there after a hand-over and a rejoin.

## 2. Parts, in the order they are built

- **N2a, the inventory.** `HOTBAR_SLOTS` becomes `INVENTORY_SLOTS = 41`: 0 to 8 the
  hotbar, 9 to 35 the main inventory, 36 to 39 armour (feet, legs, chest, head), 40 the off
  hand, as the game's `Inventory` numbers them. `PlayerState`, `Player`, `PlayerTransfer`,
  `Place` and the log's `LoggedPlace` carry all 41 (the format's layout per slot is
  unchanged; the count is). The edge maps the client's container slots (`5..=8` armour,
  `9..=35` main, `36..=44` hotbar, `45` off hand) to these and back in
  `SetContainerContent`, and `SetCreativeModeSlot` sets any of them
  (`PlayerInput::SetInventorySlot`, renamed from `SetHotbarSlot`). Independent of the rest.
- **N2b, entities through commits and the store.** The types of section 3 in
  `clustine-world`; their encoding in `clustine-format` (a commit kind and an entities
  file); the store saving, loading, replaying, copying and folding them; the worker
  putting a tick's entity changes into its commit and saving entities as their own part.
  Tested without the sim's entities: tests build `Change::Entity` by hand.
- **N2c, the sim and the edge.** Entities in the sim's chunks, their place in the tick,
  the item thrown by Q and by the creative tab, entity changes in `TickOutput`, entities
  in snapshots and events; the edge showing an entity other than a player by key and
  `hops` (spawn with type and data, removal), with `SetEntityData` for the item's stack.
- **N2d, the measurement**, as R8's: items thrown and lying, and items handed back and
  forth across a pinned line (which N3 makes possible; until then the merge and split
  costs), the log's bytes a second and a commit.

## 3. Shared types (clustine-world)

```rust
/// An entity other than a player, as its chunk keeps it (ADR-0028, section 2).
pub struct Entity {
    pub key: EntityId,
    /// Its type, an index into `clustine_data::ENTITY_TYPES`.
    pub kind: u16,
    pub uuid: Uuid,
    pub position: [f64; 3],
    pub motion: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    /// Changes of holder behind it (ADR-0028, section 6).
    pub hops: u32,
    /// The tick of its region it was made in: ages are derived from it (section 3).
    pub born: u64,
    pub data: EntityData,
}

pub enum EntityData {
    Item {
        stack: ItemStack,
        /// The tick from which it may be picked up (the game's pick-up delay, 40 for Q).
        pick_up_from: u64,
        /// Who threw it, if a player did.
        thrower: Option<Uuid>,
    },
}

pub enum EntityChange {
    /// The entity as it is after the tick: new to the chunk.
    Whole(Entity),
    /// It left the chunk.
    Gone(EntityId),
    /// Fields the tick changed, each to a definite value.
    Set(EntityId, Vec<EntityField>),
}

pub enum EntityField {
    Position([f64; 3]),
    Motion([f64; 3]),
    Rotation { yaw: f32, pitch: f32 },
    OnGround(bool),
    Data(EntityData),
}
```

`ItemStack` moves from the sim's `api.rs` to `clustine-world` (it is `{ item: i32, count:
i32 }`; components come later). `Change` gains `Entity(ChunkPos, EntityChange)`.
`Chunk` gains `entities: Vec<Entity>`, ascending by key, with `Chunk::apply` doing the
three: `Whole` replaces or inserts by key, `Gone` removes, `Set` sets the fields of a key
the chunk has and passes over one it has not. An entity's chunk is
`ChunkPos::containing(position)` (floor of x and z over 16), and a `Whole` whose position
is not in its chunk is refused by `apply` as malformed. `Chunk::for_viewers` keeps
entities out (the edge has them as `EntityState`). Bounds: `MAX_CHUNK_ENTITIES = 4096`
entities in a chunk (an entity beyond it is not added, as the game's chunk would hold more
but nothing ordinary makes that many); an entity's encoding is under 4 KiB until data with
contents comes.

## 4. Encoding (clustine-format)

- **An entity** is written as: key u64, kind u16, uuid u128, position 3 x f64, motion
  3 x f64, yaw f32, pitch f32, flags u8 (bit 0 on ground), hops u32, born u64, then the
  data: a tag u8 (1 item) and its fields (item i32, count i32, pick-up-from u64, a u8
  whether a thrower follows, the thrower u128).
- **A field** is a tag u8 (1 position, 2 motion, 3 rotation, 4 on ground, 5 data) and its
  value as above.
- **A commit with entity changes** is a new kind, `KIND_COMMIT_WITH_ENTITY_CHANGES = 15`
  (kind 13 is with block entities), the fields of kind 14 then the entity changes: a
  count of chunks (u32), per chunk x i32, z i32 and a count of changes, per change a tag
  u8 (1 whole, 2 gone, 3 set) and its body, in the order the tick made them. A commit
  with none is of the kinds it always was; one with an empty list is refused, so a
  commit has one form. Entity changes are not interleaved with block changes: what a
  tick does to blocks and to entities touch different parts of the chunk.
- **The entities file** of a chunk: version, kind 7 (`KIND_ENTITIES`), x i32, z i32, tick
  u64 (as of which it was saved), epoch u64, a count and the entities ascending by key,
  CRC-32. A chunk without entities has none, and a save of a chunk whose entities are all
  gone removes it.

## 5. The store

- **Paths**: `chunks/overworld/{x>>5}.{z>>5}/{x}.{z}.entities` beside the chunk's file.
- **Saving**: `StoreRequest::Save` gains `parts: Parts { blocks: bool, entities: bool }`.
  The worker says which parts changed since the chunk was last saved; the store writes
  only those files, in the same round (`Round::put`, renamed into place together), so
  that a chunk's two files are of one save. A save of blocks only leaves the entities
  file as it is, and the other way round. The worker sends `parts` all true for a chunk
  saved for the first time and at a checkpoint at stop.
- **Loading** reads both; a missing entities file is no entities. A load waits for an
  earlier save of the same chunk, as now (`Before`).
- **Replay** at a restore, **copies** (bases and updates), and the **fold** at a change of
  division go through `Chunk::apply` as block changes do; entity changes are filtered by
  chunk with the same rule (`replayed`: held now, tick after `held_from`), and count as
  the chunk's changes for a copy's gap check.
- **Copy updates** carry a chunk's entity changes of a commit beside its block changes.

## 6. The worker

- A tick's `TickOutput::entity_changes: Vec<(ChunkPos, EntityChange)>` go into the commit
  (filtered to loaded chunks, as ticks are), make it needed (`needs_commit`), and mark
  their chunks' entities unsaved (`unsaved` becomes per part).
- Snapshots carry the chunk's entities as `EntityState`s, made after the tick's changes
  (ADR-0028, section 6).

## 7. The sim (N2c)

- Entities live in their chunks (`Chunk::entities`); the region keeps an index from key
  to chunk, rebuilt when a chunk is loaded or taken in.
- In the tick (ADR-0024 section 6a, as ADR-0028 section 2 says): after block events, the
  entities, in ascending key; in N2 an item does nothing in its tick (no physics yet), so
  it commits nothing while it lies.
- **Throwing** (`PlayerInput::Drop { all }` from `PlayerAction` 4 and 5, and
  `PlayerInput::Throw { stack }` from `SetCreativeModeSlot` slot -1): the game's
  `Player.drop` and `LivingEntity.createItemStackToDrop`: an item entity at the eye
  height less 0.3, the motion the game gives (its three draws from the entity's stream,
  ADR-0028 section 4, with `StrictMath.sin`/`cos`, the named exception), the pick-up
  delay of 40 ticks and the thrower; Q takes one from the held stack, ctrl+Q the stack;
  nothing is thrown from an empty hand. In creative the held stack is not lessened by Q
  (`Player.drop` with `hasInfiniteMaterials`: **to check against the game** in the jar).
- The new entity takes the region's next key, a `Whole` in its chunk, and an
  `EntitySpawned` event; with no key left (a block used up, ADR-0028 section 1) nothing is
  thrown.
- `EntityKind` gains `Other { kind: u16, uuid, data, hops }`; `EntityState` stays
  `{ entity, kind, pose }`.

## 8. The edge (N2c)

- An entity other than a player is shown by key with the highest `hops`
  (ADR-0028, section 6); players keep today's rule. In N2 nothing crosses, so `hops` only
  rises at loads; the rule is built and tested now.
- **Spawn**: `SpawnEntity` with its type, UUID, position, rotation, its motion (the codec's
  velocity, which only takes zero today, is written as the game's `LpVec3`), then
  `SetEntityData` with the item's stack (the item entity's data index 8, a slot), as the
  official server sends them; checked against the official server with the comparisons
  (a client summoning an item), as packets are.
- **Removal** as now, by wire id.

## 9. Tests

- From the record and this plan, by someone who did not write the code: for N2b, an entity
  survives a crash at every step of a commit and a save (replay, a save of one part, the
  store's restart), a merge, a split, a lend and a return, a change of division; copies
  get entity changes and their gaps are seen; for N2c, a thrown item is where the game
  puts it, survives a restore, and is shown to every client in view by one wire id.

## 10. Questions for the reviewer

- Is a save by parts safe against the replay rule (replay from `held_from`, not from the
  file's tick), and against a crash between the two files' writes in one round?
- Does carrying entities inside `Chunk` (in memory) conflict with anything that clones or
  sends chunks (snapshots, copies, generation, the edge's replica)?
- Is `Change::Entity` in the same list as block changes, in tick order, a problem for
  `split_changes` and `join_changes`, or for consumers that expect blocks only?
- What does the official server send for a thrown item, exactly, in protocol 777?

## 11. The review (2026-10-18), and what changes

Ready after fixes; these replace what the sections above say where they differ.

**Order and crates.** A shared-types commit first: section 3's types, and the `Inventory`
type (below). Then N2a (the inventory, which raises `FORMAT_VERSION` to 4: `LoggedPlace` is
in the log and in the region files, so an N1 world is made again), then N2b and N2c in
parallel: N2b owns `clustine-format`, `clustine-rpc`'s store messages, the store and the
worker; N2c owns the sim, `clustine-protocol` and the edge.

**Entity changes are a field of their own**, not a variant of `Change`:
`entity_changes: Vec<(ChunkPos, EntityChange)>` in `StoreRequest::Commit`,
`LogRecord::Commit` (kind 15, which reads events with `kind >= 14` and allows empty events
and block entities) and copy updates; `Chunk::apply_entity(EntityChange)` beside
`Chunk::apply`. So `Change::position()`, `split_changes` and `join_changes` are untouched.
In a commit and in replay, a tick's block changes are applied before its entity changes.

**Counters are relative.** An entity's ages and delays (`age`, `pick_up_delay`) are stored,
in commits, copies and files, as values as of the record's tick; the region keeps them in
its own ticks while the entity is in memory and rebases them at every take-in (a load, an
arrival, a split's warm chunk, a merge's absorbed chunk) and when an entity resumes
ticking after a pause (a `Set`). Region tick counts are not shared (a merge continues the
survivor's), and the game freezes an entity's counters while it does not tick. A resting
entity commits nothing, so after a crash its counters are as of its last record: as if its
chunk had been unloaded for the ticks since, a named difference. `Entity.born` and
`pick_up_from` become `age: u32` and `pick_up_delay: u16` in the encodings; the sim's own
form may differ.

**Saving by parts** (section 5) holds, with:
- a second save of a chunk while the first is pending keeps the union of their parts and
  encodes both from the newest chunk (`FileChunks::save` keeps one per chunk);
- the removal of an entities file is a step of the round, after the renames and before
  the directory sync;
- a restore's and a fold's saves name the parts they replayed; a checkpoint and a return's
  last save name every part changed since its last save;
- an entity change to a chunk that is still as generated clears that (`clear_generated`,
  as ticks do) and marks its blocks part unsaved too, so that a chunk is not remade over
  its entities (`remakes`).

**`Chunk`** carries entities in its transport form (`ChunkRepr`) and its equality (by the
floats' bits), not in `for_viewers` or `shows_the_same`.

**Limits and refusals are the sim's.** `apply_entity` never drops; a `Whole` outside its
named chunk is refused when decoding. The sim checks before it changes anything: a throw
from a chunk the region does not have loaded and ticking (being handed over, lending), with
no key left, or into a full chunk (`MAX_CHUNK_ENTITIES`) throws nothing and puts the slot
back on the client. The worker asserts that no entity change is for a chunk not loaded.

**`hops`** rise in the `Whole` committed at every load (a restore's loads too) and for a
split part's warm chunks, which marks the chunk's entities unsaved; not at a merge. The UUID
is drawn from the stream at the entity's birth tick and draw 0, so it is stable.

**The throw, as the game's** (`ServerPlayer.drop`, `LivingEntity.createItemStackToDrop`):
- status 5 drops one, status 4 (ctrl+Q) the stack; creative does lessen the held stack
  (`Inventory.removeFromSelected`); spectators drop nothing;
- at the player's x and z and `eye y - 0.30000001192092896`, the eye height 1.27 when
  crouching: the edge passes `crouching` with `PlayerInput::Drop`;
- motion from **four** draws of the **thrower's** stream: `a = f*2π`, `d = 0.02*f`, then
  `f - f` for y; x = `(float)(-sin(yaw)*cos(pitch)*0.3f) + cos(a)*d`, y =
  `(float)(-sin(pitch)*0.3f + 0.1f + (f - f)*0.1f)`, z = `(float)(cos(yaw)*cos(pitch)*0.3f)
  + sin(a)*d`, with `Mth`'s table for yaw and pitch (`clustine-noise/src/trig.rs`) and
  `StrictMath` for `a` (the named exception); the item's own yaw is its own stream's
  `nextFloat()*360`;
- pick-up delay 40, the thrower kept; the thrower's arm swings for others (`Animate`);
- `SetCreativeModeSlot` with any negative slot drops, if the item exists and the count is
  within its maximum stack (from the data), through the game's `dropSpamThrottler`, ported
  in the edge; slots 1 to 4 (the crafting grid) are passed over, as now.

**Inventory.** `Inventory`, a type of 41 slots with serde of its own (arrays above 32 do not
derive). The container order of armour is reversed: container 5 is inventory 39 (head),
6 is 38, 7 is 37, 8 is 36; 9 to 35 are 9 to 35, 36 to 44 are 0 to 8, 45 is 40. **F** (status
6, swap with the off hand) is built. The region tells the edge of an inventory change it
made itself (F, a refused throw) by a `RegionEvent::InventorySlot { player, slot, stack }`,
which the edge sends as `SetPlayerInventory`.

**The edge's packets**: `SpawnEntity` with the item type, UUID, position, the motion as the
game's `LpVec3` (a real writer; `put_velocity` takes only zero today), pitch 0, the item's
yaw, head yaw 0, data 0; then `SetEntityData` with index 8, serializer 7 (`ITEM_STACK`), the
slot, `0xFF`. Compared with the official server by layout, index and serializer, not
values (the motion is random), with a throw by `PlayerAction`. `EntityKind::Other` carries
the motion; `RegionEvent::EntityRemoved` gains `hops` (ADR-0028, section 6).

**What a player meets until N4** (written into the roadmap's "where the work stands"): a
thrown item falls on the thrower's screen, since the client moves items itself, while the
server keeps it where it was thrown; a client that comes into view later sees it there,
in the air; items do not despawn. N4 brings their fall, landing, pick-up and despawning.
Older gaps listed: armour and held items are not shown on others (`SetEquipment`), and
middle-click pick-block does nothing.

**The record**: the entities file is `chunks/overworld/{x>>5}.{z>>5}/{x}.{z}.entities`
beside the chunk's; ADR-0028 section 2 is amended to say so.
