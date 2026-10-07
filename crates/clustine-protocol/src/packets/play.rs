//! The play state.
//!
//! Only the packets Clustine uses are modelled; the rest decode as `Unhandled`.

use super::configuration::{Disconnect as ConfigurationDisconnect, empty_packet, keep_alive};
use super::{Packet, packet_set};
use crate::codec::{Decode, DecodeError, Encode, Position, Reader, Writer};
use crate::nbt::Nbt;
use crate::packet_ids::play::{clientbound, serverbound};

/// Game mode ids as used in [`Login`].
pub mod game_mode {
    pub const SURVIVAL: i32 = 0;
    pub const CREATIVE: i32 = 1;
    pub const ADVENTURE: i32 = 2;
    pub const SPECTATOR: i32 = 3;
}

/// Where a player died, shown by the recovery compass.
#[derive(Debug, Clone, PartialEq)]
pub struct DeathLocation {
    pub dimension: String,
    pub position: Position,
}

/// Puts the client into the world: the first packet of the play state.
#[derive(Debug, Clone, PartialEq)]
pub struct Login {
    /// The player's entity id. Clients since 26.2 reject 0.
    pub entity_id: i32,
    pub hardcore: bool,
    /// All dimensions that exist on the server.
    pub dimension_names: Vec<String>,
    /// Unused by the client.
    pub max_players: i32,
    pub view_distance: i32,
    pub simulation_distance: i32,
    pub reduced_debug_info: bool,
    pub enable_respawn_screen: bool,
    pub limited_crafting: bool,
    /// Id in the `minecraft:dimension_type` registry as sent during configuration.
    pub dimension_type: i32,
    /// The dimension the player spawns in.
    pub dimension_name: String,
    pub hashed_seed: i64,
    /// One of the [`game_mode`] ids.
    pub game_mode: i32,
    pub previous_game_mode: Option<i32>,
    pub is_debug: bool,
    /// Flat worlds have their horizon and void fog at a different height.
    pub is_flat: bool,
    pub death_location: Option<DeathLocation>,
    pub portal_cooldown: i32,
    pub sea_level: i32,
    /// Whether the connection was authenticated; false on an offline-mode server.
    pub online_mode: bool,
    pub enforces_secure_chat: bool,
}

impl Packet for Login {
    const ID: i32 = clientbound::LOGIN;
}

impl Encode for Login {
    fn encode(&self, w: &mut Writer) {
        w.put_i32(self.entity_id);
        w.put_bool(self.hardcore);
        w.put_array(&self.dimension_names, |w, name| w.put_string(name));
        w.put_var_int(self.max_players);
        w.put_var_int(self.view_distance);
        w.put_var_int(self.simulation_distance);
        w.put_bool(self.reduced_debug_info);
        w.put_bool(self.enable_respawn_screen);
        w.put_bool(self.limited_crafting);
        w.put_var_int(self.dimension_type);
        w.put_string(&self.dimension_name);
        w.put_i64(self.hashed_seed);
        w.put_var_int(self.game_mode);
        // 0 stands for "none", so real ids are shifted by one.
        w.put_var_int(self.previous_game_mode.map_or(0, |mode| mode + 1));
        w.put_bool(self.is_debug);
        w.put_bool(self.is_flat);
        w.put_option(self.death_location.as_ref(), |w, location| {
            w.put_string(&location.dimension);
            w.put_position(location.position);
        });
        w.put_var_int(self.portal_cooldown);
        w.put_var_int(self.sea_level);
        w.put_bool(self.online_mode);
        w.put_bool(self.enforces_secure_chat);
    }
}

impl Decode for Login {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            entity_id: r.i32()?,
            hardcore: r.bool()?,
            dimension_names: r.array(Reader::identifier)?,
            max_players: r.var_int()?,
            view_distance: r.var_int()?,
            simulation_distance: r.var_int()?,
            reduced_debug_info: r.bool()?,
            enable_respawn_screen: r.bool()?,
            limited_crafting: r.bool()?,
            dimension_type: r.var_int()?,
            dimension_name: r.identifier()?,
            hashed_seed: r.i64()?,
            game_mode: r.var_int()?,
            previous_game_mode: match r.var_int()? {
                0 => None,
                shifted => Some(shifted - 1),
            },
            is_debug: r.bool()?,
            is_flat: r.bool()?,
            death_location: r.option(|r| {
                Ok(DeathLocation {
                    dimension: r.identifier()?,
                    position: r.position()?,
                })
            })?,
            portal_cooldown: r.var_int()?,
            sea_level: r.var_int()?,
            online_mode: r.bool()?,
            enforces_secure_chat: r.bool()?,
        })
    }
}

/// What the player is allowed to do, mainly depending on the game mode.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerAbilities {
    /// 0x01 invulnerable, 0x02 flying, 0x04 may fly, 0x08 breaks blocks instantly.
    pub flags: u8,
    pub flying_speed: f32,
    pub field_of_view_modifier: f32,
}

impl Packet for PlayerAbilities {
    const ID: i32 = clientbound::PLAYER_ABILITIES;
}

impl Encode for PlayerAbilities {
    fn encode(&self, w: &mut Writer) {
        w.put_u8(self.flags);
        w.put_f32(self.flying_speed);
        w.put_f32(self.field_of_view_modifier);
    }
}

impl Decode for PlayerAbilities {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            flags: r.u8()?,
            flying_speed: r.f32()?,
            field_of_view_modifier: r.f32()?,
        })
    }
}

/// Moves the player, which the client has to confirm with [`ConfirmTeleportation`].
#[derive(Debug, Clone, PartialEq)]
pub struct SynchronizePlayerPosition {
    pub teleport_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub velocity_x: f64,
    pub velocity_y: f64,
    pub velocity_z: f64,
    pub yaw: f32,
    pub pitch: f32,
    /// Marks components as relative to the current value; 0 means all are absolute.
    pub relative_flags: i32,
}

impl Packet for SynchronizePlayerPosition {
    const ID: i32 = clientbound::PLAYER_POSITION;
}

impl Encode for SynchronizePlayerPosition {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.teleport_id);
        for value in [self.x, self.y, self.z] {
            w.put_f64(value);
        }
        for value in [self.velocity_x, self.velocity_y, self.velocity_z] {
            w.put_f64(value);
        }
        w.put_f32(self.yaw);
        w.put_f32(self.pitch);
        w.put_i32(self.relative_flags);
    }
}

impl Decode for SynchronizePlayerPosition {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            teleport_id: r.var_int()?,
            x: r.f64()?,
            y: r.f64()?,
            z: r.f64()?,
            velocity_x: r.f64()?,
            velocity_y: r.f64()?,
            velocity_z: r.f64()?,
            yaw: r.f32()?,
            pitch: r.f32()?,
            relative_flags: r.i32()?,
        })
    }
}

/// Confirms a [`SynchronizePlayerPosition`] and reports where the client now is.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfirmTeleportation {
    pub teleport_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
}

impl Packet for ConfirmTeleportation {
    const ID: i32 = serverbound::ACCEPT_TELEPORTATION;
}

impl Encode for ConfirmTeleportation {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.teleport_id);
        for value in [self.x, self.y, self.z] {
            w.put_f64(value);
        }
        w.put_f32(self.yaw);
        w.put_f32(self.pitch);
    }
}

impl Decode for ConfirmTeleportation {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            teleport_id: r.var_int()?,
            x: r.f64()?,
            y: r.f64()?,
            z: r.f64()?,
            yaw: r.f32()?,
            pitch: r.f32()?,
        })
    }
}

/// Game event ids as used in [`GameEvent`].
pub mod game_event {
    /// The client may leave the loading screen once the chunk it stands in has arrived.
    pub const START_WAITING_FOR_LEVEL_CHUNKS: u8 = 13;
}

/// A state change that is not tied to an entity or block.
#[derive(Debug, Clone, PartialEq)]
pub struct GameEvent {
    /// One of the [`game_event`] ids.
    pub event: u8,
    pub value: f32,
}

impl Packet for GameEvent {
    const ID: i32 = clientbound::GAME_EVENT;
}

impl Encode for GameEvent {
    fn encode(&self, w: &mut Writer) {
        w.put_u8(self.event);
        w.put_f32(self.value);
    }
}

impl Decode for GameEvent {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            event: r.u8()?,
            value: r.f32()?,
        })
    }
}

/// Tells the client which chunk its loaded area is centred on.
#[derive(Debug, Clone, PartialEq)]
pub struct SetCenterChunk {
    pub chunk_x: i32,
    pub chunk_z: i32,
}

impl Packet for SetCenterChunk {
    const ID: i32 = clientbound::SET_CHUNK_CACHE_CENTER;
}

impl Encode for SetCenterChunk {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.chunk_x);
        w.put_var_int(self.chunk_z);
    }
}

impl Decode for SetCenterChunk {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            chunk_x: r.var_int()?,
            chunk_z: r.var_int()?,
        })
    }
}

keep_alive!(
    /// Must be answered with a [`ServerboundKeepAlive`] carrying the same id.
    ClientboundKeepAlive,
    clientbound::KEEP_ALIVE
);
keep_alive!(
    /// The answer to a [`ClientboundKeepAlive`].
    ServerboundKeepAlive,
    serverbound::KEEP_ALIVE
);

empty_packet!(
    /// Sent by the client at the end of each of its ticks.
    ClientTickEnd,
    serverbound::CLIENT_TICK_END
);

/// Ends the connection with a message shown to the player.
#[derive(Debug, Clone, PartialEq)]
pub struct Disconnect {
    /// A text component; a plain string tag is the simplest form.
    pub reason: Nbt,
}

impl Packet for Disconnect {
    const ID: i32 = clientbound::DISCONNECT;
}

impl Encode for Disconnect {
    fn encode(&self, w: &mut Writer) {
        ConfigurationDisconnect {
            reason: self.reason.clone(),
        }
        .encode_fields(w);
    }
}

impl Decode for Disconnect {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let reason = ConfigurationDisconnect::decode_fields(r)?.reason;
        Ok(Self { reason })
    }
}

/// Heightmap kinds the client uses, as ids in [`Heightmap`].
pub mod heightmap {
    /// Highest block that is not air.
    pub const WORLD_SURFACE: i32 = 1;
    /// Highest block that blocks motion or contains a fluid.
    pub const MOTION_BLOCKING: i32 = 4;
    /// Like `MOTION_BLOCKING`, ignoring leaves.
    pub const MOTION_BLOCKING_NO_LEAVES: i32 = 5;
}

/// One heightmap of a chunk; see [`crate::chunk::pack_heightmap`].
#[derive(Debug, Clone, PartialEq)]
pub struct Heightmap {
    /// One of the [`heightmap`] ids.
    pub kind: i32,
    pub data: Vec<u64>,
}

/// A block entity inside a chunk, such as a chest or a sign.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockEntity {
    /// The position within the chunk: `x << 4 | z`.
    pub packed_xz: u8,
    pub y: i16,
    /// Id in the block entity type registry.
    pub kind: i32,
    pub data: Option<Nbt>,
}

/// Light levels of a chunk column.
///
/// Light is stored per section for the sections of the column plus one below and one
/// above, from bottom to top. Bit `i` of a mask refers to the `i`-th of those. A section
/// is either listed in a light mask, with a 2048-byte array of 4-bit levels following in
/// the same order, or in the matching empty mask, meaning all zero, or in neither.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LightData {
    pub sky_mask: Vec<u64>,
    pub block_mask: Vec<u64>,
    pub empty_sky_mask: Vec<u64>,
    pub empty_block_mask: Vec<u64>,
    pub sky: Vec<Vec<u8>>,
    pub block: Vec<Vec<u8>>,
}

impl LightData {
    fn encode(&self, w: &mut Writer) {
        w.put_bit_set(&self.sky_mask);
        w.put_bit_set(&self.block_mask);
        w.put_bit_set(&self.empty_sky_mask);
        w.put_bit_set(&self.empty_block_mask);
        for arrays in [&self.sky, &self.block] {
            w.put_array(arrays, |w, array| {
                w.put_length(array.len());
                w.put_bytes(array);
            });
        }
    }

    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        fn arrays(r: &mut Reader<'_>) -> Result<Vec<Vec<u8>>, DecodeError> {
            r.array(|r| {
                let length = r.length()?;
                Ok(r.bytes(length)?.to_vec())
            })
        }
        Ok(Self {
            sky_mask: r.bit_set()?,
            block_mask: r.bit_set()?,
            empty_sky_mask: r.bit_set()?,
            empty_block_mask: r.bit_set()?,
            sky: arrays(r)?,
            block: arrays(r)?,
        })
    }
}

/// A chunk column with its light.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelChunkWithLight {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub heightmaps: Vec<Heightmap>,
    /// The sections, bottom to top; see [`crate::chunk::encode_sections`].
    pub sections: Vec<u8>,
    pub block_entities: Vec<BlockEntity>,
    pub light: LightData,
}

impl Packet for LevelChunkWithLight {
    const ID: i32 = clientbound::LEVEL_CHUNK_WITH_LIGHT;
}

impl Encode for LevelChunkWithLight {
    fn encode(&self, w: &mut Writer) {
        w.put_i32(self.chunk_x);
        w.put_i32(self.chunk_z);
        w.put_array(&self.heightmaps, |w, heightmap| {
            w.put_var_int(heightmap.kind);
            w.put_array(&heightmap.data, |w, word| w.put_u64(*word));
        });
        w.put_length(self.sections.len());
        w.put_bytes(&self.sections);
        w.put_array(&self.block_entities, |w, entity| {
            w.put_u8(entity.packed_xz);
            w.put_i16(entity.y);
            w.put_var_int(entity.kind);
            match &entity.data {
                Some(data) => w.put_nbt(data),
                None => w.put_u8(0),
            }
        });
        self.light.encode(w);
    }
}

impl Decode for LevelChunkWithLight {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            chunk_x: r.i32()?,
            chunk_z: r.i32()?,
            heightmaps: r.array(|r| {
                Ok(Heightmap {
                    kind: r.var_int()?,
                    data: r.array(Reader::u64)?,
                })
            })?,
            sections: {
                let length = r.length()?;
                r.bytes(length)?.to_vec()
            },
            block_entities: r.array(|r| {
                Ok(BlockEntity {
                    packed_xz: r.u8()?,
                    y: r.i16()?,
                    kind: r.var_int()?,
                    data: r.nbt()?,
                })
            })?,
            light: LightData::decode(r)?,
        })
    }
}

empty_packet!(
    /// Announces a batch of chunk packets, which ends with [`ChunkBatchFinished`].
    ChunkBatchStart,
    clientbound::CHUNK_BATCH_START
);

/// Ends a batch of chunk packets. The client answers with [`ChunkBatchReceived`].
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkBatchFinished {
    /// Number of chunks in the batch.
    pub batch_size: i32,
}

impl Packet for ChunkBatchFinished {
    const ID: i32 = clientbound::CHUNK_BATCH_FINISHED;
}

impl Encode for ChunkBatchFinished {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.batch_size);
    }
}

impl Decode for ChunkBatchFinished {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            batch_size: r.var_int()?,
        })
    }
}

/// Confirms a chunk batch and tells the server how fast the client can take chunks.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkBatchReceived {
    pub chunks_per_tick: f32,
}

impl Packet for ChunkBatchReceived {
    const ID: i32 = serverbound::CHUNK_BATCH_RECEIVED;
}

impl Encode for ChunkBatchReceived {
    fn encode(&self, w: &mut Writer) {
        w.put_f32(self.chunks_per_tick);
    }
}

impl Decode for ChunkBatchReceived {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            chunks_per_tick: r.f32()?,
        })
    }
}

packet_set! {
    /// Packets a client can send in the play state.
    pub enum ServerboundPlay in crate::packet_ids::play::serverbound {
        ConfirmTeleportation,
        ServerboundKeepAlive,
        ClientTickEnd,
        ChunkBatchReceived,
    }
}

packet_set! {
    /// Packets a server can send in the play state.
    pub enum ClientboundPlay in crate::packet_ids::play::clientbound {
        Login,
        PlayerAbilities,
        SynchronizePlayerPosition,
        GameEvent,
        SetCenterChunk,
        ClientboundKeepAlive,
        Disconnect,
        LevelChunkWithLight,
        ChunkBatchStart,
        ChunkBatchFinished,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::packets::testing::assert_round_trip;

    fn login() -> Login {
        Login {
            entity_id: 42,
            hardcore: false,
            dimension_names: vec!["minecraft:overworld".to_owned()],
            max_players: 20,
            view_distance: 10,
            simulation_distance: 10,
            reduced_debug_info: false,
            enable_respawn_screen: true,
            limited_crafting: false,
            dimension_type: 0,
            dimension_name: "minecraft:overworld".to_owned(),
            hashed_seed: 0x1234_5678_9ABC_DEF0,
            game_mode: game_mode::CREATIVE,
            previous_game_mode: None,
            is_debug: false,
            is_flat: true,
            death_location: None,
            portal_cooldown: 0,
            sea_level: 63,
            online_mode: false,
            enforces_secure_chat: false,
        }
    }

    #[test]
    fn clientbound_packets_round_trip() {
        use ClientboundPlay as Set;
        assert_round_trip(login(), Set::decode, Set::Login);
        assert_round_trip(
            Login {
                previous_game_mode: Some(game_mode::SURVIVAL),
                death_location: Some(DeathLocation {
                    dimension: "minecraft:the_nether".to_owned(),
                    position: Position { x: -5, y: 70, z: 9 },
                }),
                ..login()
            },
            Set::decode,
            Set::Login,
        );
        assert_round_trip(
            PlayerAbilities {
                flags: 0x0F,
                flying_speed: 0.05,
                field_of_view_modifier: 0.1,
            },
            Set::decode,
            Set::PlayerAbilities,
        );
        assert_round_trip(
            SynchronizePlayerPosition {
                teleport_id: 1,
                x: 0.5,
                y: -60.0,
                z: 0.5,
                velocity_x: 0.0,
                velocity_y: 0.0,
                velocity_z: 0.0,
                yaw: 90.0,
                pitch: 0.0,
                relative_flags: 0,
            },
            Set::decode,
            Set::SynchronizePlayerPosition,
        );
        assert_round_trip(
            GameEvent {
                event: game_event::START_WAITING_FOR_LEVEL_CHUNKS,
                value: 0.0,
            },
            Set::decode,
            Set::GameEvent,
        );
        assert_round_trip(
            SetCenterChunk {
                chunk_x: -3,
                chunk_z: 7,
            },
            Set::decode,
            Set::SetCenterChunk,
        );
        assert_round_trip(
            ClientboundKeepAlive { id: 99 },
            Set::decode,
            Set::ClientboundKeepAlive,
        );
        assert_round_trip(
            Disconnect {
                reason: Nbt::String("bye".to_owned()),
            },
            Set::decode,
            Set::Disconnect,
        );
    }

    #[test]
    fn serverbound_packets_round_trip() {
        use ServerboundPlay as Set;
        assert_round_trip(
            ConfirmTeleportation {
                teleport_id: 1,
                x: 0.5,
                y: -60.0,
                z: 0.5,
                yaw: 90.0,
                pitch: 0.0,
            },
            Set::decode,
            Set::ConfirmTeleportation,
        );
        assert_round_trip(
            ServerboundKeepAlive { id: 99 },
            Set::decode,
            Set::ServerboundKeepAlive,
        );
        assert_round_trip(ClientTickEnd, Set::decode, Set::ClientTickEnd);
    }

    #[test]
    fn chunk_packets_round_trip() {
        use ClientboundPlay as Set;
        assert_round_trip(
            LevelChunkWithLight {
                chunk_x: -2,
                chunk_z: 5,
                heightmaps: vec![Heightmap {
                    kind: heightmap::MOTION_BLOCKING,
                    data: vec![1, 2, 3],
                }],
                sections: vec![0, 1, 2, 3, 4],
                block_entities: vec![
                    BlockEntity {
                        packed_xz: 0x4A,
                        y: -60,
                        kind: 7,
                        data: Some(Nbt::Compound(vec![("a".to_owned(), Nbt::Int(1))])),
                    },
                    BlockEntity {
                        packed_xz: 0,
                        y: 0,
                        kind: 1,
                        data: None,
                    },
                ],
                light: LightData {
                    sky_mask: vec![0b110],
                    block_mask: vec![],
                    empty_sky_mask: vec![0b001],
                    empty_block_mask: vec![0b111],
                    sky: vec![vec![0xFF; 2048], vec![0x0F; 2048]],
                    block: vec![],
                },
            },
            Set::decode,
            Set::LevelChunkWithLight,
        );
        assert_round_trip(ChunkBatchStart, Set::decode, Set::ChunkBatchStart);
        assert_round_trip(
            ChunkBatchFinished { batch_size: 9 },
            Set::decode,
            Set::ChunkBatchFinished,
        );
        assert_round_trip(
            ChunkBatchReceived {
                chunks_per_tick: 20.0,
            },
            ServerboundPlay::decode,
            ServerboundPlay::ChunkBatchReceived,
        );
    }

    #[test]
    fn previous_game_mode_is_shifted_by_one() {
        let none = crate::packets::encode(&login());
        let survival = crate::packets::encode(&Login {
            previous_game_mode: Some(game_mode::SURVIVAL),
            ..login()
        });
        // The two encodings differ in exactly that one byte: 0 for none, 1 for survival.
        let differing: Vec<_> = none.iter().zip(&survival).filter(|(a, b)| a != b).collect();
        assert_eq!(differing, [(&0, &1)]);
    }

    proptest! {
        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = ServerboundPlay::decode(&bytes);
            let _ = ClientboundPlay::decode(&bytes);
        }
    }
}
