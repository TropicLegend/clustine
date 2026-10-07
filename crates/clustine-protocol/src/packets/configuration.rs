//! The configuration state: registries, tags and settings exchanged before play.

use super::{Packet, packet_set};
use crate::codec::{Decode, DecodeError, Encode, MAX_STRING_LENGTH, Reader, Writer};
use crate::nbt::Nbt;
use crate::packet_ids::configuration::{clientbound, serverbound};

/// The settings from the client's options screen.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientInformation {
    /// For example `en_us`.
    pub locale: String,
    /// The render distance in chunks.
    pub view_distance: i8,
    /// 0 enabled, 1 commands only, 2 hidden.
    pub chat_mode: i32,
    pub chat_colors: bool,
    /// A bit mask of the outer skin layers the player shows.
    pub displayed_skin_parts: u8,
    /// 0 left, 1 right.
    pub main_hand: i32,
    pub text_filtering: bool,
    pub allow_server_listing: bool,
    /// 0 all, 1 decreased, 2 minimal.
    pub particle_status: i32,
}

impl ClientInformation {
    pub(crate) fn encode_fields(&self, w: &mut Writer) {
        w.put_string(&self.locale);
        w.put_i8(self.view_distance);
        w.put_var_int(self.chat_mode);
        w.put_bool(self.chat_colors);
        w.put_u8(self.displayed_skin_parts);
        w.put_var_int(self.main_hand);
        w.put_bool(self.text_filtering);
        w.put_bool(self.allow_server_listing);
        w.put_var_int(self.particle_status);
    }

    pub(crate) fn decode_fields(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            locale: r.string(16)?,
            view_distance: r.i8()?,
            chat_mode: r.var_int()?,
            chat_colors: r.bool()?,
            displayed_skin_parts: r.u8()?,
            main_hand: r.var_int()?,
            text_filtering: r.bool()?,
            allow_server_listing: r.bool()?,
            particle_status: r.var_int()?,
        })
    }
}

impl Packet for ClientInformation {
    const ID: i32 = serverbound::CLIENT_INFORMATION;
}

impl Encode for ClientInformation {
    fn encode(&self, w: &mut Writer) {
        self.encode_fields(w);
    }
}

impl Decode for ClientInformation {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Self::decode_fields(r)
    }
}

/// Defines a packet that carries a channel name and opaque data for it.
macro_rules! plugin_message {
    ($(#[$meta:meta])* $name:ident, $id:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name {
            /// For example `minecraft:brand`.
            pub channel: String,
            pub data: Vec<u8>,
        }

        impl Packet for $name {
            const ID: i32 = $id;
        }

        impl Encode for $name {
            fn encode(&self, w: &mut Writer) {
                w.put_string(&self.channel);
                w.put_bytes(&self.data);
            }
        }

        impl Decode for $name {
            fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
                Ok(Self {
                    channel: r.identifier()?,
                    data: r.rest().to_vec(),
                })
            }
        }
    };
}

plugin_message!(
    /// Data on a named channel, from the client.
    ServerboundPluginMessage,
    serverbound::CUSTOM_PAYLOAD
);
plugin_message!(
    /// Data on a named channel, from the server.
    ClientboundPluginMessage,
    clientbound::CUSTOM_PAYLOAD
);

/// Defines a packet that carries a keep-alive id.
macro_rules! keep_alive {
    ($(#[$meta:meta])* $name:ident, $id:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name {
            pub id: i64,
        }

        impl Packet for $name {
            const ID: i32 = $id;
        }

        impl Encode for $name {
            fn encode(&self, w: &mut Writer) {
                w.put_i64(self.id);
            }
        }

        impl Decode for $name {
            fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
                Ok(Self { id: r.i64()? })
            }
        }
    };
}
pub(crate) use keep_alive;

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

/// Defines a packet without fields.
macro_rules! empty_packet {
    ($(#[$meta:meta])* $name:ident, $id:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name;

        impl Packet for $name {
            const ID: i32 = $id;
        }

        impl Encode for $name {
            fn encode(&self, _: &mut Writer) {}
        }

        impl Decode for $name {
            fn decode(_: &mut Reader<'_>) -> Result<Self, DecodeError> {
                Ok(Self)
            }
        }
    };
}
pub(crate) use empty_packet;

empty_packet!(
    /// Ends configuration. The client validates the registries and tags it received.
    FinishConfiguration,
    clientbound::FINISH_CONFIGURATION
);
empty_packet!(
    /// Confirms [`FinishConfiguration`] and switches the connection to the play state.
    AcknowledgeFinishConfiguration,
    serverbound::FINISH_CONFIGURATION
);

/// A data pack both sides may already have, so that its contents need not be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownPack {
    pub namespace: String,
    pub id: String,
    pub version: String,
}

/// Defines a packet that carries a list of known packs.
macro_rules! known_packs {
    ($(#[$meta:meta])* $name:ident, $id:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name {
            pub packs: Vec<KnownPack>,
        }

        impl Packet for $name {
            const ID: i32 = $id;
        }

        impl Encode for $name {
            fn encode(&self, w: &mut Writer) {
                w.put_array(&self.packs, |w, pack| {
                    w.put_string(&pack.namespace);
                    w.put_string(&pack.id);
                    w.put_string(&pack.version);
                });
            }
        }

        impl Decode for $name {
            fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
                let packs = r.array(|r| {
                    Ok(KnownPack {
                        namespace: r.string(MAX_STRING_LENGTH)?,
                        id: r.string(MAX_STRING_LENGTH)?,
                        version: r.string(MAX_STRING_LENGTH)?,
                    })
                })?;
                Ok(Self { packs })
            }
        }
    };
}

known_packs!(
    /// The packs the server offers to rely on.
    ClientboundKnownPacks,
    clientbound::SELECT_KNOWN_PACKS
);
known_packs!(
    /// The offered packs the client has, in the order they were offered.
    ServerboundKnownPacks,
    serverbound::SELECT_KNOWN_PACKS
);

/// The feature flags that are enabled, normally just `minecraft:vanilla`.
#[derive(Debug, Clone, PartialEq)]
pub struct FeatureFlags {
    pub features: Vec<String>,
}

impl Packet for FeatureFlags {
    const ID: i32 = clientbound::UPDATE_ENABLED_FEATURES;
}

impl Encode for FeatureFlags {
    fn encode(&self, w: &mut Writer) {
        w.put_array(&self.features, |w, feature| w.put_string(feature));
    }
}

impl Decode for FeatureFlags {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            features: r.array(Reader::identifier)?,
        })
    }
}

/// One entry of a synchronised registry.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryEntry {
    pub id: String,
    /// The entry's definition. If absent, the client takes it from a known pack.
    pub data: Option<Nbt>,
}

/// The entries of one synchronised registry. Their order assigns the numeric ids.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryData {
    pub registry: String,
    pub entries: Vec<RegistryEntry>,
}

impl Packet for RegistryData {
    const ID: i32 = clientbound::REGISTRY_DATA;
}

impl Encode for RegistryData {
    fn encode(&self, w: &mut Writer) {
        w.put_string(&self.registry);
        w.put_array(&self.entries, |w, entry| {
            w.put_string(&entry.id);
            w.put_option(entry.data.as_ref(), |w, data| w.put_nbt(data));
        });
    }
}

impl Decode for RegistryData {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            registry: r.identifier()?,
            entries: r.array(|r| {
                Ok(RegistryEntry {
                    id: r.identifier()?,
                    data: r.option(Reader::nbt)?.flatten(),
                })
            })?,
        })
    }
}

/// A tag and the numeric ids of its members.
#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub name: String,
    pub entries: Vec<i32>,
}

/// All tags of one registry.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryTags {
    pub registry: String,
    pub tags: Vec<Tag>,
}

/// The tags of every registry that has any.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateTags {
    pub registries: Vec<RegistryTags>,
}

impl Packet for UpdateTags {
    const ID: i32 = clientbound::UPDATE_TAGS;
}

impl Encode for UpdateTags {
    fn encode(&self, w: &mut Writer) {
        w.put_array(&self.registries, |w, registry| {
            w.put_string(&registry.registry);
            w.put_array(&registry.tags, |w, tag| {
                w.put_string(&tag.name);
                w.put_array(&tag.entries, |w, entry| w.put_var_int(*entry));
            });
        });
    }
}

impl Decode for UpdateTags {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let registries = r.array(|r| {
            Ok(RegistryTags {
                registry: r.identifier()?,
                tags: r.array(|r| {
                    Ok(Tag {
                        name: r.identifier()?,
                        entries: r.array(Reader::var_int)?,
                    })
                })?,
            })
        })?;
        Ok(Self { registries })
    }
}

/// Ends the connection with a message shown to the player.
#[derive(Debug, Clone, PartialEq)]
pub struct Disconnect {
    /// A text component; a plain string tag is the simplest form.
    pub reason: Nbt,
}

impl Disconnect {
    pub(crate) fn encode_fields(&self, w: &mut Writer) {
        w.put_nbt(&self.reason);
    }

    pub(crate) fn decode_fields(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let reason = r.nbt()?.ok_or(DecodeError::InvalidValue {
            what: "text component",
            value: 0,
        })?;
        Ok(Self { reason })
    }
}

impl Packet for Disconnect {
    const ID: i32 = clientbound::DISCONNECT;
}

impl Encode for Disconnect {
    fn encode(&self, w: &mut Writer) {
        self.encode_fields(w);
    }
}

impl Decode for Disconnect {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Self::decode_fields(r)
    }
}

packet_set! {
    /// Packets a client can send in the configuration state.
    pub enum ServerboundConfiguration in crate::packet_ids::configuration::serverbound {
        ClientInformation,
        ServerboundPluginMessage,
        ServerboundKeepAlive,
        ServerboundKnownPacks,
        AcknowledgeFinishConfiguration,
    }
}

packet_set! {
    /// Packets a server can send in the configuration state.
    pub enum ClientboundConfiguration in crate::packet_ids::configuration::clientbound {
        ClientboundPluginMessage,
        Disconnect,
        FinishConfiguration,
        ClientboundKeepAlive,
        RegistryData,
        FeatureFlags,
        UpdateTags,
        ClientboundKnownPacks,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::packets::encode;
    use crate::packets::testing::assert_round_trip;

    fn core_pack() -> Vec<KnownPack> {
        vec![KnownPack {
            namespace: "minecraft".to_owned(),
            id: "core".to_owned(),
            version: "26.3".to_owned(),
        }]
    }

    #[test]
    fn serverbound_packets_round_trip() {
        use ServerboundConfiguration as Set;
        assert_round_trip(
            ClientInformation {
                locale: "en_us".to_owned(),
                view_distance: 12,
                chat_mode: 0,
                chat_colors: true,
                displayed_skin_parts: 0x7F,
                main_hand: 1,
                text_filtering: false,
                allow_server_listing: true,
                particle_status: 2,
            },
            Set::decode,
            Set::ClientInformation,
        );
        assert_round_trip(
            ServerboundPluginMessage {
                channel: "minecraft:brand".to_owned(),
                data: b"\x07vanilla".to_vec(),
            },
            Set::decode,
            Set::ServerboundPluginMessage,
        );
        assert_round_trip(
            ServerboundKeepAlive { id: -5 },
            Set::decode,
            Set::ServerboundKeepAlive,
        );
        assert_round_trip(
            ServerboundKnownPacks { packs: core_pack() },
            Set::decode,
            Set::ServerboundKnownPacks,
        );
        assert_round_trip(
            AcknowledgeFinishConfiguration,
            Set::decode,
            Set::AcknowledgeFinishConfiguration,
        );
    }

    #[test]
    fn clientbound_packets_round_trip() {
        use ClientboundConfiguration as Set;
        assert_round_trip(
            ClientboundPluginMessage {
                channel: "minecraft:brand".to_owned(),
                data: b"\x08clustine".to_vec(),
            },
            Set::decode,
            Set::ClientboundPluginMessage,
        );
        assert_round_trip(
            Disconnect {
                reason: Nbt::String("bye".to_owned()),
            },
            Set::decode,
            Set::Disconnect,
        );
        assert_round_trip(FinishConfiguration, Set::decode, Set::FinishConfiguration);
        assert_round_trip(
            ClientboundKeepAlive { id: 7 },
            Set::decode,
            Set::ClientboundKeepAlive,
        );
        assert_round_trip(
            RegistryData {
                registry: "minecraft:dimension_type".to_owned(),
                entries: vec![
                    RegistryEntry {
                        id: "minecraft:overworld".to_owned(),
                        data: None,
                    },
                    RegistryEntry {
                        id: "minecraft:custom".to_owned(),
                        data: Some(Nbt::Compound(vec![("height".to_owned(), Nbt::Int(384))])),
                    },
                ],
            },
            Set::decode,
            Set::RegistryData,
        );
        assert_round_trip(
            FeatureFlags {
                features: vec!["minecraft:vanilla".to_owned()],
            },
            Set::decode,
            Set::FeatureFlags,
        );
        assert_round_trip(
            UpdateTags {
                registries: vec![RegistryTags {
                    registry: "minecraft:block".to_owned(),
                    tags: vec![Tag {
                        name: "minecraft:infiniburn_overworld".to_owned(),
                        entries: vec![3, 300],
                    }],
                }],
            },
            Set::decode,
            Set::UpdateTags,
        );
        assert_round_trip(
            ClientboundKnownPacks { packs: core_pack() },
            Set::decode,
            Set::ClientboundKnownPacks,
        );
    }

    #[test]
    fn registry_entry_without_data_known_answer() {
        let packet = RegistryData {
            registry: "a:b".to_owned(),
            entries: vec![RegistryEntry {
                id: "a:c".to_owned(),
                data: None,
            }],
        };
        let mut expected = vec![clientbound::REGISTRY_DATA as u8, 3];
        expected.extend_from_slice(b"a:b");
        expected.extend_from_slice(&[1, 3]);
        expected.extend_from_slice(b"a:c");
        expected.push(0);
        assert_eq!(encode(&packet), expected);
    }

    #[test]
    fn disconnect_needs_a_reason() {
        assert!(matches!(
            ClientboundConfiguration::decode(&[clientbound::DISCONNECT as u8, 0]),
            Err(DecodeError::InvalidValue { .. })
        ));
    }

    proptest! {
        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = ServerboundConfiguration::decode(&bytes);
            let _ = ClientboundConfiguration::decode(&bytes);
        }
    }
}
