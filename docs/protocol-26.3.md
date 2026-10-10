# Protocol notes for Minecraft: Java Edition 26.3

Working notes for the packets Clustine needs in milestone M1. Protocol version **777**.
Packet numeric ids are not listed here; they come from the generated tables.

Collected on 2026-10-07 from the Minecraft Wiki's
[Java Edition protocol](https://minecraft.wiki/w/Java_Edition_protocol) pages, the 26.3
version manifest, the official 26.3 jars and ViaVersion's 26.x translation code. The wiki
is behind 26.3 in places; where a note says "jar", it was read from the game itself.
Items that could not be confirmed are listed in [Unverified](#unverified) and are settled
by running the same bot scenarios against the official server.

Names are the wiki's, with the `packets.json` name in parentheses where it differs.
C→S is serverbound, S→C is clientbound.

## Versions

| Version | Protocol |
|---|---|
| 26.3 | 777 |
| 26.2 | 776 |
| 26.1, 26.1.1, 26.1.2 | 775 |
| 1.21.11 | 774 |

The server jar for 26.3 is 62,294,556 bytes, needs Java 25, and is a bundler jar: the
data generator is started with `-DbundlerMainClass=net.minecraft.data.Main`.

## Join sequence

### Status (separate connection)

1. C→S Handshake (`intention`), intent 1
2. C→S Status Request
3. S→C Status Response
4. C→S Ping Request
5. S→C Pong Response with the same value, then close

Status Response is a string of JSON with five optional top-level keys:

```json
{
  "version": { "name": "26.3", "protocol": 777 },
  "players": { "max": 20, "online": 0, "sample": [] },
  "description": { "text": "A Clustine server" },
  "favicon": "data:image/png;base64,...",
  "enforcesSecureChat": false
}
```

The client still has a legacy ping (`FE 01 ...`) that it falls back to when the modern
ping gets no proper response, including malformed JSON.

### Login

| Direction | Packet | Notes |
|---|---|---|
| C→S | Handshake (`intention`) | Protocol 777, intent 2 (3 is transfer) |
| C→S | Login Start (`hello`) | Name (16), UUID |
| S→C / C→S | Encryption Request / Response | Online mode only |
| S→C | Set Compression (`login_compression`) | Optional; must come before Login Success |
| S→C | Login Success (`login_finished`) | Game profile (UUID, name, properties) and, since 26.2, a session id UUID |
| C→S | Login Acknowledged | Switches to configuration |

Vanilla disconnects a connection that stays in login for 600 ticks.

### Configuration

| Direction | Packet | Needed | Notes |
|---|---|---|---|
| C→S | Plugin Message `minecraft:brand`, Client Information | | Sent by the client unprompted |
| S→C | Plugin Message `minecraft:brand` | Optional | |
| S→C | Feature Flags (`update_enabled_features`) | Optional | Vanilla sends `minecraft:vanilla` |
| S→C | Known Packs (`select_known_packs`) | Expected | Offer `minecraft` / `core` / `26.3` |
| C→S | Known Packs | | Vanilla waits for it |
| S→C | Registry Data, one per registry | Required | See [Registries](#registries) |
| S→C | Update Tags | Required in practice | See [Tags](#tags) |
| S→C | Finish Configuration | Required | The client validates registries and tags here |
| C→S | Acknowledge Finish Configuration | | Switches to play |

### Play

Order used by the vanilla server:

| Direction | Packet | Needed | Notes |
|---|---|---|---|
| S→C | Login (`login`) | Required | Layout below |
| S→C | Player Abilities | Expected | Flags: 0x01 invulnerable, 0x02 flying, 0x04 may fly, 0x08 instant break |
| S→C | Synchronize Player Position (`player_position`) | Required in practice | |
| C→S | Confirm Teleportation (`accept_teleportation`) | | Carries position and rotation in 26.3 |
| S→C | Player Info Update | Expected | Existing players to the joiner, then the joiner to everyone including itself |
| S→C | Game Event 13, "start waiting for level chunks" | Required | |
| S→C | Set Center Chunk (`set_chunk_cache_center`) | Expected | |
| S→C | Chunk Batch Start, chunks, Chunk Batch Finished | Required | At least the chunk the player stands in |
| C→S | Chunk Batch Received | | Float: chunks per tick the client wants |
| C→S | Player Loaded | | Sent when the loading screen closes |

The loading screen closes after game event 13 once the player is in a loaded chunk, or
after 30 seconds regardless.

Login (play) fields in 26.3, in order:

| Field | Type |
|---|---|
| Entity id | Int |
| Hardcore | Boolean |
| Dimension names | Array of Identifier |
| Max players | VarInt |
| View distance | VarInt |
| Simulation distance | VarInt |
| Reduced debug info | Boolean |
| Enable respawn screen | Boolean |
| Limited crafting | Boolean |
| Dimension type | VarInt registry id |
| Dimension name | Identifier |
| Hashed seed | Long |
| Game mode | VarInt |
| Previous game mode | Optional VarInt (0 is none, otherwise id + 1) |
| Is debug | Boolean |
| Is flat | Boolean |
| Death location | Optional (Identifier, Position) |
| Portal cooldown | VarInt |
| Sea level | VarInt |
| Online mode | Boolean (since 26.2; false in offline mode) |
| Enforces secure chat | Boolean |

### Ongoing obligations

- **Keep-alive:** vanilla sends one every 15 seconds and disconnects if the previous one
  is unanswered at the next interval or the echoed id differs. It exists in configuration
  and play.
- **Read timeout:** client and server both drop a connection that is silent for 30 seconds.
- **Teleports:** vanilla ignores movement until the matching confirmation arrives. The
  26.3 client sends only the confirmation, without a follow-up position packet.
- **Chunk batches:** vanilla starts at 9 chunks per tick, allows one unacknowledged batch
  until the first acknowledgement and then 10, and clamps the client's wish to 0.01–64.
- **Client Tick End:** sent every client tick, no fields, no reply.
- **Movement:** the client sends a position at least every 20 ticks even when standing still.
- **Set Center Chunk** whenever the player crosses a chunk border. The client keeps a
  square of 2 × view distance + 7 chunks. Unload Chunk is Z then X.
- The client only reliably renders a chunk whose neighbours are loaded.

## Registries

With known packs the server may send entry ids without NBT; the client takes the content
from its own copy of the pack. Vanilla only omits NBT if the client's reply equals the
offered list. Every entry in use must still be listed, and **list order assigns the
numeric ids**, starting at 0. Tags never come from known packs.

The 32 synchronised registries of 26.3, in vanilla's order, with vanilla entry counts:

| # | Registry | Entries | # | Registry | Entries |
|---|---|---|---|---|---|
| 1 | `worldgen/biome` | 67 | 17 | `painting_variant` | 51 |
| 2 | `chat_type` | 7 | 18 | `sulfur_cube_archetype` | 12 |
| 3 | `trim_pattern` | 18 | 19 | `dimension_type` | 4 |
| 4 | `trim_material` | 11 | 20 | `damage_type` | 51 |
| 5 | `wolf_variant` | 9 | 21 | `banner_pattern` | 43 |
| 6 | `wolf_sound_variant` | 7 | 22 | `enchantment` | 43 |
| 7 | `pig_variant` | 3 | 23 | `jukebox_song` | 22 |
| 8 | `pig_sound_variant` | 3 | 24 | `instrument` | 8 |
| 9 | `frog_variant` | 3 | 25 | `test_environment` | 1 |
| 10 | `cat_variant` | 11 | 26 | `test_instance` | 1 |
| 11 | `cat_sound_variant` | 2 | 27 | `dialog` | 3 |
| 12 | `cow_sound_variant` | 2 | 28 | `world_clock` | 2 |
| 13 | `cow_variant` | 3 | 29 | `timeline` | 4 |
| 14 | `chicken_sound_variant` | 2 | 30 | `decorated_pot_pattern` | 23 |
| 15 | `chicken_variant` | 3 | 31 | `block_transformer` | 3 |
| 16 | `zombie_nautilus_variant` | 2 | 32 | `worldgen/block_state_provider` | 8 |

The last three are new in 26.3. Clustine lists every vanilla entry of every registry.

## Tags

Since 26.1 the client refuses to finish configuration without certain tags, because
vanilla registry entries and default item components reference them (for example
`dimension_type/overworld` refers to `#minecraft:infiniburn_overworld`). Clustine sends
all vanilla tags of every registry the client knows: block, item, fluid, entity type,
game event, point of interest type, potion, and the synchronised registries that have tags.

## Chunk Data and Update Light (`level_chunk_with_light`)

| Field | Type |
|---|---|
| Chunk X, Chunk Z | Int, Int |
| Heightmaps | Array of (VarInt type, Array of Long) |
| Data | Byte array holding the sections |
| Block entities | Array of (packed XZ byte, Short Y, VarInt type, NBT) |
| Sky light mask, block light mask, empty sky mask, empty block mask | BitSet × 4, each a byte array (see below) |
| Sky light arrays, block light arrays | Array of 2048-byte arrays, each |

A bit set is sent as a byte count followed by its bytes, lowest bits first, with trailing
zero bytes left out; for example bits 9 and 10 are `02 00 06`. This was observed from the
official 26.3 server and differs from the wiki, which describes an array of 64-bit words.

Heightmap types the client uses: 1 world surface, 4 motion blocking, 5 motion blocking
without leaves. Each has 256 entries of `ceil(log2(height + 1))` bits, which is 9 bits and
37 longs for the overworld. An empty heightmap array is accepted.

A section, sent bottom to top without a count:

| Field | Type |
|---|---|
| Non-air block count | Short |
| Fluid count (since 26.1) | Short |
| Block states | Paletted container, 4096 entries |
| Biomes | Paletted container, 64 entries |

A paletted container is a bits-per-entry byte, then the palette, then longs **without a
length prefix**. The number of longs is `ceil(entries / floor(64 / bits))`; entries never
span two longs.

| | Single value | Indirect | Direct |
|---|---|---|---|
| Blocks | 0 bits, one VarInt, no data | 4–8 bits (1–3 round up to 4) | 9 bits or more: no palette, global ids |
| Biomes | 0 bits, one VarInt, no data | 1–3 bits | 4 bits or more: no palette |

The direct width is `ceil(log2(number of entries in the registry))`. With 35,723 block
states in 26.3 that is **16 bits** for blocks; with 67 biomes it is 7 bits.

The overworld has `min_y = -64` and a height of 384, so 24 sections. Light masks cover
sections + 2 = 26 bits: bit 0 is the section below the world. The official server lists
dark sections in the empty mask, sends arrays for sections that contain light up to one
section above the highest block, and lists the fully sky-lit sections above that in
neither mask. Each light array is prefixed with its length, 2048.

Update Light (`light_update`) is VarInt X, VarInt Z and the same light data.

## Movement

Serverbound, all ending in a flags byte (0x01 on ground, 0x02 horizontal collision):

| Packet | Fields before the flags |
|---|---|
| Set Player Position (`move_player_pos`) | X, feet Y, Z as Double |
| Set Player Position and Rotation (`move_player_pos_rot`) | X, Y, Z, then yaw and pitch as Float |
| Set Player Rotation (`move_player_rot`) | Yaw, pitch |
| Set Player Movement Flags (`move_player_status_only`) | None |

Vanilla kicks for NaN or infinite coordinates.

Since 26.3 the official server also accepts at most one packet with a position per client
tick: a second one before the next Client Tick End is answered with the disconnect
"invalid player movement". A client therefore has to end each tick in which it moved with
Client Tick End. (Read from the server's movement handler; the wiki does not mention it.)

The official server also puts a client back where it was if it moves before it has sent
Player Loaded, which a client sends when it leaves the loading screen. (Observed: a bot
that walked without sending it was teleported back once; with it, never.)

The chunks a client is sent are not a square. With view distance `d`, a chunk at offset
`(dx, dz)` from the client's chunk is in view if
`max(0, |dx| - 2)² + max(0, |dz| - 2)² < d²`, which gives 329 chunks for `d = 8`.

Synchronize Player Position (`player_position`): VarInt teleport id, position (3 Double),
velocity (3 Double), yaw, pitch (Float), Int flags marking relative components (X 0x01,
Y 0x02, Z 0x04, yaw 0x08, pitch 0x10, velocity 0x20/0x40/0x80, rotate delta 0x100).

Confirm Teleportation in 26.3: VarInt id, position (3 Double), yaw, pitch (Float).

## Abilities

| Direction | Packet | Id | Layout |
|---|---|---|---|
| S→C | Player Abilities (`player_abilities`) | 65 | Flags byte, flying speed and field-of-view modifier as Float |
| C→S | Player Abilities (`player_abilities`) | 40 | Flags byte |

The ids are those of the game's packets report (`generated/packet_ids.rs`). The flags
are 0x01 invulnerable, 0x02 flying, 0x04 may fly, 0x08 instant break. A client sends
its packet when its player begins or stops flying and sets nothing but 0x02 in it; a
server answers nothing. The codec has it as `ServerboundPlayerAbilities`, which is not
among `ServerboundPlay` until the edge reads it (ADR-0020, R1.4).

## Text components and the reason of a disconnect

Disconnect (`disconnect`) in the configuration and play states carries one text
component as nameless NBT; in the login state (`login_disconnect`) it is a JSON string.
A component is a plain string tag, shown as it is, or a compound. The compound
`{translate: "<key>"}` names a sentence of the game, which each client shows in its own
language: `multiplayer.disconnect.duplicate_login` is "You logged in from another
location". The codec reads and writes the two forms as `text::Text` and keeps any other
component as the NBT it came as.

## Blocks

Player Action (`player_action`): VarInt status, Position, face byte, VarInt sequence.
Faces: 0 −Y, 1 +Y, 2 −Z, 3 +Z, 4 −X, 5 +X.

Status values in 26.3 (from the jar; the wiki table still shows the 26.2 numbering):

| Value | Status |
|---|---|
| 0 | Start destroying block |
| 1 | Change destroy direction (new in 26.3) |
| 2 | Abort destroying block |
| 3 | Stop destroying block |
| 4 | Drop all items |
| 5 | Drop item |
| 6 | Release use item |
| 7 | Swap item with off hand |
| 8 | Stab |

In creative mode the client sends only status 0 and assumes the block is gone.

Acknowledge Block Change (`block_changed_ack`) is one VarInt; vanilla sends the highest
handled sequence once per tick.

Use Item On (`use_item_on`): VarInt hand (0 main, 1 off), Position, VarInt face, cursor
X, Y, Z as Float in 0..1, Boolean inside block, Boolean world border hit, VarInt sequence.

Block Update (`block_update`): Position, VarInt state id.

Update Section Blocks (`section_blocks_update`): Long section position
(`x << 42 | z << 20 | y`), then an array of VarLong (`state << 12 | x << 8 | z << 4 | y`).

Set Held Item: serverbound (`set_carried_item`) is a Short, clientbound (`set_held_slot`)
a VarInt.

Set Creative Mode Slot (`set_creative_mode_slot`): Short slot and an item stack.

An item stack is a VarInt count, and unless that is 0: a VarInt item id, the number of
added components, the number of removed components, the added components (VarInt type
and value) and the removed ones (VarInt type). Servers write component values bare.
Clients prefix each value with its length, so a server can skip components it does not
know. A plain block is count, item id, 0, 0.

In 26.3 the serverbound Swing Arm packet is gone. The client sends Punch (`punch`, no
fields) and the server broadcasts Swing Animation (`swing_animation`): VarInt entity id,
VarInt hand, VarInt type (0 none, 1 whack, 2 stab), VarInt duration.

## Other players

Order: Player Info Update with "add player", then Spawn Entity, then optional metadata,
then movement. Never spawn the receiving player itself, and **never use entity id 0**.

Player Info Update (`player_info_update`): a mask byte, then an array of (UUID, data for
each set action in bit order).

| Bit | Action | Data |
|---|---|---|
| 0x01 | Add player | Name (16), properties |
| 0x02 | Initialise chat | Optional chat session |
| 0x04 | Update game mode | VarInt |
| 0x08 | Update listed | Boolean |
| 0x10 | Update latency | VarInt |
| 0x20 | Update display name | Optional text component |
| 0x40 | Update list order | VarInt |
| 0x80 | Update hat | Boolean |

Spawn Entity (`add_entity`): VarInt entity id, UUID (the profile's), VarInt entity type,
position (3 Double), velocity (a single zero byte when at rest), pitch, yaw, head yaw as
angle bytes, VarInt data.

Entity metadata entries are index byte, VarInt type, value, ended by `0xFF`. None is
mandatory. Index 16 holds the displayed skin parts and defaults to 0, which hides the
outer skin layers.

Movement of other entities changed in 26.3:

| Packet | Layout |
|---|---|
| Update Entity Position (`move_entity_pos`) | VarInt id, VarInt properties, deltas |
| Update Entity Position and Rotation (`move_entity_pos_rot`) | The same, then yaw and pitch bytes |
| Update Entity Rotation (`move_entity_rot`) | VarInt id, Boolean on ground, yaw and pitch bytes |
| Teleport Entity (`entity_position_sync`) | VarInt id, VarInt path type, path, yaw and pitch as Float, Boolean on ground |
| Set Head Rotation (`rotate_head`) | VarInt id, head yaw byte |

In `properties`, bit 0 is "on ground" and the higher bits are a step count. With a step
count of 0 the deltas are three Shorts (delta × 4096); with N steps they are N × (VarInt
tick offset, three Shorts). Path type 0 is three Doubles; path type 1 is an array of
(three Doubles, VarInt tick offset). The head only turns if Set Head Rotation is sent.

Removal: Remove Entities (array of VarInt) and Player Info Remove (array of UUID).

## Changes since 1.21.x that affect the above

- **26.1:** fluid count in chunk sections; Update Time carries world clocks; tags and
  entries referenced by default item components became mandatory; attack is its own
  packet; Java 25; the game is no longer obfuscated.
- **26.2:** session id in Login Success; online mode flag in Login (play); entity id 0 is
  rejected; `sulfur_cube_archetype` registry.
- **26.3:** position in Confirm Teleportation; renumbered Player Action statuses; Punch
  and Swing Animation; new entity movement layout; VarInt game modes in Login (play);
  three new synchronised registries; 16-bit direct block palette.

## Unverified

- Whether the client applies a 20-second keep-alive rule in addition to the 30-second read timeout.
- Whether the client accepts chunks sent outside a chunk batch.
- Which of the three registries added in 26.3 may be empty.
- The exact set of tags the 26.3 client requires; sending all vanilla tags avoids the question.
- That the direct block palette is 16 bits: computed from the state count, not observed.
- The encoding of item stacks with component changes in Set Creative Mode Slot.
- That entity id 0 is rejected: taken from a ViaVersion comment.
- That the serverbound Player Abilities is one byte in which 0x02 is flying, that a
  second login as a connected name ends the first connection with
  `{translate: "multiplayer.disconnect.duplicate_login"}` and nothing more in the
  compound, and that a player who left flying is sent 0x02 before their position on
  entering again: from earlier versions, not observed. The last three tests of
  `bin/clustine/tests/oracle.rs` ask the official server (ADR-0020, R1.0c).
- Lighting behaviour with empty masks, and the neighbour-chunk rendering rule: wiki notes from 1.20.x.
