//! The status state: the server list ping.

use super::{Packet, packet_set};
use crate::codec::{Decode, DecodeError, Encode, MAX_STRING_LENGTH, Reader, Writer};
use crate::packet_ids::status::{clientbound, serverbound};

/// Asks for the server list entry.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusRequest;

impl Packet for StatusRequest {
    const ID: i32 = serverbound::STATUS_REQUEST;
}

impl Encode for StatusRequest {
    fn encode(&self, _: &mut Writer) {}
}

impl Decode for StatusRequest {
    fn decode(_: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self)
    }
}

/// Asks the server to echo `payload`, which the client uses to measure latency.
#[derive(Debug, Clone, PartialEq)]
pub struct PingRequest {
    pub payload: i64,
}

impl Packet for PingRequest {
    const ID: i32 = serverbound::PING_REQUEST;
}

impl Encode for PingRequest {
    fn encode(&self, w: &mut Writer) {
        w.put_i64(self.payload);
    }
}

impl Decode for PingRequest {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self { payload: r.i64()? })
    }
}

/// The server list entry as a JSON document.
///
/// Its top-level keys are `version`, `players`, `description`, `favicon` and
/// `enforcesSecureChat`; see `docs/protocol-26.3.md`.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusResponse {
    pub json: String,
}

impl Packet for StatusResponse {
    const ID: i32 = clientbound::STATUS_RESPONSE;
}

impl Encode for StatusResponse {
    fn encode(&self, w: &mut Writer) {
        w.put_string(&self.json);
    }
}

impl Decode for StatusResponse {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            json: r.string(MAX_STRING_LENGTH)?,
        })
    }
}

/// Echoes the payload of a [`PingRequest`].
#[derive(Debug, Clone, PartialEq)]
pub struct PongResponse {
    pub payload: i64,
}

impl Packet for PongResponse {
    const ID: i32 = clientbound::PONG_RESPONSE;
}

impl Encode for PongResponse {
    fn encode(&self, w: &mut Writer) {
        w.put_i64(self.payload);
    }
}

impl Decode for PongResponse {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self { payload: r.i64()? })
    }
}

packet_set! {
    /// Packets a client can send in the status state.
    pub enum ServerboundStatus in crate::packet_ids::status::serverbound {
        StatusRequest,
        PingRequest,
    }
}

packet_set! {
    /// Packets a server can send in the status state.
    pub enum ClientboundStatus in crate::packet_ids::status::clientbound {
        StatusResponse,
        PongResponse,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::packets::encode;
    use crate::packets::testing::assert_round_trip;

    #[test]
    fn known_answers() {
        assert_eq!(encode(&StatusRequest), [0x00]);
        assert_eq!(
            encode(&PingRequest { payload: 1 }),
            [0x01, 0, 0, 0, 0, 0, 0, 0, 1]
        );
        assert_eq!(
            encode(&StatusResponse {
                json: "{}".to_owned()
            }),
            [0x00, 0x02, b'{', b'}']
        );
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        assert_eq!(
            ServerboundStatus::decode(&[0x00, 0xFF]),
            Err(DecodeError::TrailingBytes(1))
        );
    }

    proptest! {
        #[test]
        fn packets_round_trip(payload: i64, json in ".{0,200}") {
            assert_round_trip(StatusRequest, ServerboundStatus::decode, ServerboundStatus::StatusRequest);
            assert_round_trip(
                PingRequest { payload },
                ServerboundStatus::decode,
                ServerboundStatus::PingRequest,
            );
            assert_round_trip(
                StatusResponse { json },
                ClientboundStatus::decode,
                ClientboundStatus::StatusResponse,
            );
            assert_round_trip(
                PongResponse { payload },
                ClientboundStatus::decode,
                ClientboundStatus::PongResponse,
            );
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = ServerboundStatus::decode(&bytes);
            let _ = ClientboundStatus::decode(&bytes);
        }
    }
}
