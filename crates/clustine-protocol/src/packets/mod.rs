//! Packet definitions, grouped by protocol state.
//!
//! Each packet is a struct implementing [`Packet`]. For every state and direction there
//! is an enum of the packets that can arrive, such as [`status::ServerboundStatus`],
//! whose `decode` turns an unframed packet into the matching struct.

pub mod handshake;
pub mod login;
pub mod status;

use crate::codec::{Decode, Encode, Writer};

/// A packet with a fixed id within one protocol state and direction.
pub trait Packet: Encode + Decode {
    const ID: i32;
}

/// Encodes `packet` as its id followed by its body, ready for framing.
pub fn encode<P: Packet>(packet: &P) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.put_var_int(P::ID);
    packet.encode(&mut writer);
    writer.into_bytes()
}

/// Defines the enum of packets that can arrive in one protocol state and direction.
///
/// `$ids` is the generated module of that state and direction; it tells ids the game
/// defines but this crate does not model apart from ids that do not exist.
macro_rules! packet_set {
    (
        $(#[$meta:meta])*
        pub enum $name:ident in $ids:path {
            $($packet:ident),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq)]
        pub enum $name {
            $($packet($packet),)+
            /// A packet that exists in this state but is not modelled by this crate.
            Unhandled { id: i32 },
        }

        impl $name {
            /// Decodes an unframed packet: its id followed by its body.
            pub fn decode(frame: &[u8]) -> Result<Self, $crate::codec::DecodeError> {
                use $ids as ids;
                use $crate::codec::{Decode, DecodeError, Reader};
                use $crate::packets::Packet;

                let mut reader = Reader::new(frame);
                let id = reader.var_int()?;
                $(
                    if id == <$packet as Packet>::ID {
                        let packet = <$packet as Decode>::decode(&mut reader)?;
                        reader.finish()?;
                        return Ok(Self::$packet(packet));
                    }
                )+
                if usize::try_from(id).is_ok_and(|id| id < ids::NAMES.len()) {
                    Ok(Self::Unhandled { id })
                } else {
                    Err(DecodeError::UnknownPacket(id))
                }
            }
        }
    };
}
pub(crate) use packet_set;

#[cfg(test)]
pub(crate) mod testing {
    use std::fmt::Debug;

    use super::{Packet, encode};

    /// Asserts that `packet` encodes to something `decode` turns back into `expected`.
    pub fn assert_round_trip<P, S>(
        packet: P,
        decode: impl Fn(&[u8]) -> Result<S, crate::codec::DecodeError>,
        wrap: impl Fn(P) -> S,
    ) where
        P: Packet,
        S: Debug + PartialEq,
    {
        let bytes = encode(&packet);
        assert_eq!(decode(&bytes), Ok(wrap(packet)));
    }
}
