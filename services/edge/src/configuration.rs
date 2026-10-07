//! The configuration state: sending the client the registries and tags of the game.
//!
//! Clustine does not ship the contents of the vanilla data pack. It lists every registry
//! entry by name and relies on the client taking the definitions from its own copy of
//! the pack, which the "known packs" exchange establishes.

use clustine_data::{GAME_VERSION, SYNCED_REGISTRIES, TAGS};
use clustine_protocol::codec::Writer;
use clustine_protocol::nbt::Nbt;
use clustine_protocol::packets;
use clustine_protocol::packets::configuration::{
    ClientInformation, ClientboundKnownPacks, ClientboundPluginMessage, Disconnect, FeatureFlags,
    FinishConfiguration, KnownPack, RegistryData, RegistryEntry, RegistryTags,
    ServerboundConfiguration, Tag, UpdateTags,
};

use crate::Shared;
use crate::connection::{Connection, ConnectionError};

/// The name the client shows for this server software, for example on the debug screen.
const BRAND: &str = "Clustine";

/// The data pack whose contents the client is expected to have.
fn core_pack() -> KnownPack {
    KnownPack {
        namespace: "minecraft".to_owned(),
        id: "core".to_owned(),
        version: GAME_VERSION.to_owned(),
    }
}

/// The packets that are the same for every client, encoded once: the brand, the feature
/// flags and the offer to rely on the core pack.
pub(crate) fn opening_packets() -> Vec<Vec<u8>> {
    let mut brand = Writer::new();
    brand.put_string(BRAND);
    vec![
        packets::encode(&ClientboundPluginMessage {
            channel: "minecraft:brand".to_owned(),
            data: brand.into_bytes(),
        }),
        packets::encode(&FeatureFlags {
            features: vec!["minecraft:vanilla".to_owned()],
        }),
        packets::encode(&ClientboundKnownPacks {
            packs: vec![core_pack()],
        }),
    ]
}

/// The packets that are the same for every client, encoded once: every synchronised
/// registry by entry name, all tags, and the end of configuration.
pub(crate) fn registry_packets() -> Vec<Vec<u8>> {
    let mut encoded: Vec<_> = SYNCED_REGISTRIES
        .iter()
        .map(|registry| {
            packets::encode(&RegistryData {
                registry: registry.name.to_owned(),
                entries: registry
                    .entries
                    .iter()
                    .map(|entry| RegistryEntry {
                        id: (*entry).to_owned(),
                        data: None,
                    })
                    .collect(),
            })
        })
        .collect();
    encoded.push(packets::encode(&UpdateTags {
        registries: TAGS
            .iter()
            .map(|registry| RegistryTags {
                registry: registry.registry.to_owned(),
                tags: registry
                    .tags
                    .iter()
                    .map(|tag| Tag {
                        name: tag.name.to_owned(),
                        entries: tag.entries.to_vec(),
                    })
                    .collect(),
            })
            .collect(),
    }));
    encoded.push(packets::encode(&FinishConfiguration));
    encoded
}

/// Takes the client through configuration. Returns the settings the client sent, if it
/// sent any, or `None` if the connection ended here.
pub(crate) async fn serve(
    connection: &mut Connection,
    shared: &Shared,
) -> Result<Option<Option<ClientInformation>>, ConnectionError> {
    for packet in &shared.opening_packets {
        connection.queue_encoded(packet)?;
    }
    connection.flush().await?;

    // The client sends its settings and brand on its own; they can arrive at any point.
    let mut client_information = None;
    let known_packs = match next(connection, &mut client_information).await? {
        Some(ServerboundConfiguration::ServerboundKnownPacks(reply)) => reply.packs,
        Some(_) => return Err(ConnectionError::Protocol("expected the known packs")),
        None => return Ok(None),
    };
    if known_packs != [core_pack()] {
        let reason = format!("This server needs an unmodified Minecraft {GAME_VERSION} client.");
        let reason = Nbt::String(reason);
        connection.write(&Disconnect { reason }).await?;
        connection.close_gracefully().await;
        return Ok(None);
    }

    for packet in &shared.registry_packets {
        connection.queue_encoded(packet)?;
    }
    connection.flush().await?;
    match next(connection, &mut client_information).await? {
        Some(ServerboundConfiguration::AcknowledgeFinishConfiguration(_)) => {
            Ok(Some(client_information))
        }
        Some(_) => Err(ConnectionError::Protocol(
            "expected the end of configuration",
        )),
        None => Ok(None),
    }
}

/// Waits for the next packet that drives configuration forward. Settings are stored in
/// `client_information`; the brand and packets Clustine has no use for are skipped.
async fn next(
    connection: &mut Connection,
    client_information: &mut Option<ClientInformation>,
) -> Result<Option<ServerboundConfiguration>, ConnectionError> {
    loop {
        let Some(frame) = connection.read_frame().await? else {
            return Ok(None);
        };
        match ServerboundConfiguration::decode(&frame)? {
            ServerboundConfiguration::ClientInformation(information) => {
                *client_information = Some(information);
            }
            ServerboundConfiguration::ServerboundPluginMessage(_)
            | ServerboundConfiguration::Unhandled { .. } => {}
            packet => return Ok(Some(packet)),
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_protocol::packets::configuration::ClientboundConfiguration;

    use super::*;

    #[test]
    fn registry_packets_list_every_entry_without_data() {
        let packets = registry_packets();
        // One per registry, then the tags, then the end of configuration.
        assert_eq!(packets.len(), SYNCED_REGISTRIES.len() + 2);

        for (packet, registry) in packets.iter().zip(&SYNCED_REGISTRIES) {
            let Ok(ClientboundConfiguration::RegistryData(data)) =
                ClientboundConfiguration::decode(packet)
            else {
                panic!("expected registry data for {}", registry.name);
            };
            assert_eq!(data.registry, registry.name);
            assert_eq!(data.entries.len(), registry.entries.len());
            assert!(data.entries.iter().all(|entry| entry.data.is_none()));
        }
        assert!(matches!(
            ClientboundConfiguration::decode(&packets[packets.len() - 2]),
            Ok(ClientboundConfiguration::UpdateTags(tags)) if tags.registries.len() == TAGS.len()
        ));
        assert_eq!(
            ClientboundConfiguration::decode(&packets[packets.len() - 1]),
            Ok(ClientboundConfiguration::FinishConfiguration(
                FinishConfiguration
            ))
        );
    }
}
