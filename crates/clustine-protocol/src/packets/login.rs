//! The login state.

use uuid::Uuid;

use super::{Packet, packet_set};
use crate::codec::{Decode, DecodeError, Encode, MAX_STRING_LENGTH, Reader, Writer};
use crate::packet_ids::login::{clientbound, serverbound};

/// The longest JSON text component the client accepts in [`LoginDisconnect`].
const MAX_REASON_LENGTH: usize = 262144;

/// The longest player name.
pub const MAX_NAME_LENGTH: usize = 16;

/// Starts the login with the name and UUID the client claims.
#[derive(Debug, Clone, PartialEq)]
pub struct LoginStart {
    pub name: String,
    /// Not trusted by the server: an offline-mode server derives the UUID from the name.
    pub uuid: Uuid,
}

impl Packet for LoginStart {
    const ID: i32 = serverbound::HELLO;
}

impl Encode for LoginStart {
    fn encode(&self, w: &mut Writer) {
        w.put_string(&self.name);
        w.put_uuid(self.uuid);
    }
}

impl Decode for LoginStart {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            name: r.string(MAX_NAME_LENGTH)?,
            uuid: r.uuid()?,
        })
    }
}

/// Confirms [`LoginSuccess`] and switches the connection to the configuration state.
#[derive(Debug, Clone, PartialEq)]
pub struct LoginAcknowledged;

impl Packet for LoginAcknowledged {
    const ID: i32 = serverbound::LOGIN_ACKNOWLEDGED;
}

impl Encode for LoginAcknowledged {
    fn encode(&self, _: &mut Writer) {}
}

impl Decode for LoginAcknowledged {
    fn decode(_: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self)
    }
}

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

/// Switches both directions to the compressed packet format, effective immediately
/// after this packet. Packets of at least `threshold` bytes are compressed.
#[derive(Debug, Clone, PartialEq)]
pub struct SetCompression {
    pub threshold: i32,
}

impl Packet for SetCompression {
    const ID: i32 = clientbound::LOGIN_COMPRESSION;
}

impl Encode for SetCompression {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.threshold);
    }
}

impl Decode for SetCompression {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            threshold: r.var_int()?,
        })
    }
}

/// A property of a player profile, such as the skin.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileProperty {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}

impl ProfileProperty {
    pub(crate) fn encode(&self, w: &mut Writer) {
        w.put_string(&self.name);
        w.put_string(&self.value);
        w.put_option(self.signature.as_ref(), |w, signature| {
            w.put_string(signature)
        });
    }

    pub(crate) fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            name: r.string(64)?,
            value: r.string(MAX_STRING_LENGTH)?,
            signature: r.option(|r| r.string(1024))?,
        })
    }
}

/// Accepts the login and tells the client its final profile.
#[derive(Debug, Clone, PartialEq)]
pub struct LoginSuccess {
    pub uuid: Uuid,
    pub name: String,
    pub properties: Vec<ProfileProperty>,
    /// Identifies this login; added in 26.2.
    pub session_id: Uuid,
}

impl Packet for LoginSuccess {
    const ID: i32 = clientbound::LOGIN_FINISHED;
}

impl Encode for LoginSuccess {
    fn encode(&self, w: &mut Writer) {
        w.put_uuid(self.uuid);
        w.put_string(&self.name);
        w.put_array(&self.properties, |w, property| property.encode(w));
        w.put_uuid(self.session_id);
    }
}

impl Decode for LoginSuccess {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            uuid: r.uuid()?,
            name: r.string(MAX_NAME_LENGTH)?,
            properties: r.array(ProfileProperty::decode)?,
            session_id: r.uuid()?,
        })
    }
}

packet_set! {
    /// Packets a client can send in the login state.
    pub enum ServerboundLogin in crate::packet_ids::login::serverbound {
        LoginStart,
        LoginAcknowledged,
    }
}

packet_set! {
    /// Packets a server can send in the login state.
    pub enum ClientboundLogin in crate::packet_ids::login::clientbound {
        LoginDisconnect,
        SetCompression,
        LoginSuccess,
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

    #[test]
    fn names_longer_than_16_characters_are_rejected() {
        let bytes = crate::packets::encode(&LoginStart {
            name: "a".repeat(17),
            uuid: Uuid::nil(),
        });
        assert_eq!(
            ServerboundLogin::decode(&bytes),
            Err(DecodeError::StringTooLong { max: 16 })
        );
    }

    proptest! {
        #[test]
        fn packets_round_trip(
            name in "[a-zA-Z0-9_]{1,16}",
            uuid: u128,
            session_id: u128,
            threshold: i32,
            signature in prop::option::of(".{0,40}"),
            text in ".{0,200}",
        ) {
            let uuid = Uuid::from_u128(uuid);
            assert_round_trip(
                LoginStart { name: name.clone(), uuid },
                ServerboundLogin::decode,
                ServerboundLogin::LoginStart,
            );
            assert_round_trip(
                LoginAcknowledged,
                ServerboundLogin::decode,
                ServerboundLogin::LoginAcknowledged,
            );
            assert_round_trip(
                LoginDisconnect { reason_json: text.clone() },
                ClientboundLogin::decode,
                ClientboundLogin::LoginDisconnect,
            );
            assert_round_trip(
                SetCompression { threshold },
                ClientboundLogin::decode,
                ClientboundLogin::SetCompression,
            );
            let properties = vec![ProfileProperty {
                name: "textures".to_owned(),
                value: text,
                signature,
            }];
            assert_round_trip(
                LoginSuccess { uuid, name, properties, session_id: Uuid::from_u128(session_id) },
                ClientboundLogin::decode,
                ClientboundLogin::LoginSuccess,
            );
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = ServerboundLogin::decode(&bytes);
            let _ = ClientboundLogin::decode(&bytes);
        }
    }
}
