//! Minecraft: Java Edition wire protocol: packet definitions and codec.
//!
//! This crate performs no I/O. [`frame`] splits a byte stream into packets, [`packets`]
//! defines them, and [`codec`] reads and writes the values inside them. Decoding is safe
//! on untrusted input.

pub mod codec;
pub mod frame;
pub mod nbt;
pub mod packets;

#[rustfmt::skip]
mod generated;

/// Numeric packet ids by protocol state and direction, written by `cargo datagen`.
pub use generated::packet_ids;

#[cfg(test)]
mod tests {
    use super::packet_ids;

    #[test]
    fn well_known_packet_ids() {
        assert_eq!(packet_ids::handshake::serverbound::INTENTION, 0);
        assert_eq!(packet_ids::status::clientbound::STATUS_RESPONSE, 0);
        assert_eq!(packet_ids::status::clientbound::PONG_RESPONSE, 1);
        assert_eq!(packet_ids::login::serverbound::HELLO, 0);
    }

    #[test]
    fn names_are_indexed_by_id() {
        use packet_ids::play::clientbound;
        assert_eq!(
            clientbound::NAMES[clientbound::LEVEL_CHUNK_WITH_LIGHT as usize],
            "minecraft:level_chunk_with_light"
        );
    }
}
