//! The handshake state: the single packet that opens every connection.

use super::{Packet, packet_set};
use crate::codec::{Decode, DecodeError, Encode, Reader, Writer};
use crate::packet_ids::handshake::serverbound as ids;

/// What the client wants to do after the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// Query the server list entry.
    Status,
    /// Join the game.
    Login,
    /// Join after being transferred from another server.
    Transfer,
}

/// The first packet of a connection.
#[derive(Debug, Clone, PartialEq)]
pub struct Intention {
    pub protocol_version: i32,
    /// The host name the client connected to; not validated by vanilla.
    pub server_address: String,
    pub server_port: u16,
    pub intent: Intent,
}

impl Packet for Intention {
    const ID: i32 = ids::INTENTION;
}

impl Encode for Intention {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.protocol_version);
        w.put_string(&self.server_address);
        w.put_u16(self.server_port);
        w.put_var_int(match self.intent {
            Intent::Status => 1,
            Intent::Login => 2,
            Intent::Transfer => 3,
        });
    }
}

impl Decode for Intention {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            protocol_version: r.var_int()?,
            server_address: r.string(255)?,
            server_port: r.u16()?,
            intent: match r.var_int()? {
                1 => Intent::Status,
                2 => Intent::Login,
                3 => Intent::Transfer,
                other => {
                    return Err(DecodeError::InvalidValue {
                        what: "handshake intent",
                        value: other.into(),
                    });
                }
            },
        })
    }
}

packet_set! {
    /// Packets a client can send in the handshake state.
    pub enum ServerboundHandshake in crate::packet_ids::handshake::serverbound {
        Intention,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::packets::encode;
    use crate::packets::testing::assert_round_trip;

    #[test]
    fn intention_known_answer() {
        let packet = Intention {
            protocol_version: 777,
            server_address: "localhost".to_owned(),
            server_port: 25565,
            intent: Intent::Status,
        };
        let mut expected = vec![0x00, 0x89, 0x06, 0x09];
        expected.extend_from_slice(b"localhost");
        expected.extend_from_slice(&[0x63, 0xDD, 0x01]);
        assert_eq!(encode(&packet), expected);
    }

    #[test]
    fn unknown_intent_is_rejected() {
        let mut bytes = encode(&Intention {
            protocol_version: 777,
            server_address: String::new(),
            server_port: 0,
            intent: Intent::Login,
        });
        *bytes.last_mut().unwrap() = 4;
        assert_eq!(
            ServerboundHandshake::decode(&bytes),
            Err(DecodeError::InvalidValue {
                what: "handshake intent",
                value: 4
            })
        );
    }

    #[test]
    fn unknown_packet_id_is_rejected() {
        assert_eq!(
            ServerboundHandshake::decode(&[0x01]),
            Err(DecodeError::UnknownPacket(1))
        );
    }

    proptest! {
        #[test]
        fn intention_round_trips(
            protocol_version: i32,
            server_address in ".{0,255}",
            server_port: u16,
            intent in prop_oneof![
                Just(Intent::Status),
                Just(Intent::Login),
                Just(Intent::Transfer),
            ],
        ) {
            // The pattern counts characters, the protocol counts UTF-16 code units.
            prop_assume!(server_address.encode_utf16().count() <= 255);
            let packet = Intention { protocol_version, server_address, server_port, intent };
            assert_round_trip(packet, ServerboundHandshake::decode, ServerboundHandshake::Intention);
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = ServerboundHandshake::decode(&bytes);
        }
    }
}
