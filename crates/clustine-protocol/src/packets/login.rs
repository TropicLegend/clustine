//! The login state.

use super::{Packet, packet_set};
use crate::codec::{Decode, DecodeError, Encode, Reader, Writer};
use crate::packet_ids::login::clientbound;

/// The longest JSON text component the client accepts here.
const MAX_REASON_LENGTH: usize = 262144;

/// Ends the connection during login with a message shown to the player.
#[derive(Debug, Clone, PartialEq)]
pub struct LoginDisconnect {
    /// A JSON text component, for example `{"text":"Server is full"}`.
    pub reason_json: String,
}

impl Packet for LoginDisconnect {
    const ID: i32 = clientbound::LOGIN_DISCONNECT;
}

impl Encode for LoginDisconnect {
    fn encode(&self, w: &mut Writer) {
        w.put_string(&self.reason_json);
    }
}

impl Decode for LoginDisconnect {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            reason_json: r.string(MAX_REASON_LENGTH)?,
        })
    }
}

packet_set! {
    /// Packets a server can send in the login state.
    pub enum ClientboundLogin in crate::packet_ids::login::clientbound {
        LoginDisconnect,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::packets::testing::assert_round_trip;

    #[test]
    fn unmodelled_packets_are_reported_as_unhandled() {
        // `hello`, the encryption request, exists but is not modelled yet.
        assert_eq!(
            ClientboundLogin::decode(&[clientbound::HELLO as u8, 0xAA]),
            Ok(ClientboundLogin::Unhandled {
                id: clientbound::HELLO
            })
        );
        assert_eq!(
            ClientboundLogin::decode(&[clientbound::NAMES.len() as u8]),
            Err(DecodeError::UnknownPacket(clientbound::NAMES.len() as i32))
        );
    }

    proptest! {
        #[test]
        fn login_disconnect_round_trips(reason_json in ".{0,200}") {
            assert_round_trip(
                LoginDisconnect { reason_json },
                ClientboundLogin::decode,
                ClientboundLogin::LoginDisconnect,
            );
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = ClientboundLogin::decode(&bytes);
        }
    }
}
