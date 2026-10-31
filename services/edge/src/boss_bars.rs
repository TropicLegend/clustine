//! Boss bars at the edge: a raid's (`docs/groundwork/n10-plan.md`, sections 9.4 and 11)
//! and the dragon's (`docs/groundwork/p-plan.md`, section 19.5 item 7), each change the
//! region reports as a `boss_event`, in the style the change carries.
//!
//! The raid's bar is `new ServerBossEvent(RAID_NAME_COMPONENT, RED, NOTCHED_10)` with the
//! three flags false, and a raid only adds, removes, and updates its progress and name
//! (`docs/groundwork/n10-dumps.md`, part C, section 3; the packet's operation classes are
//! in `n10-javap`).

use bytes::Bytes;
use clustine_protocol::nbt::Nbt;
use clustine_protocol::packets;
use clustine_protocol::packets::play::{BossEvent, BossOperation};
use clustine_sim::api::{BarChange, BarName, BarStyle, RaidBarName};
use uuid::Uuid;

/// `BossEvent.encodeProperties`: bit 0 the darkened screen, bit 1 the boss music, bit 2
/// the fog.
fn properties(style: BarStyle) -> u8 {
    u8::from(style.darken_screen)
        | (u8::from(style.play_music) << 1)
        | (u8::from(style.create_fog) << 2)
}

/// The `boss_event` of a change of the bar `id`, if it is sent.
pub(crate) fn boss_bar(id: Uuid, change: BarChange) -> Option<Bytes> {
    let operation = match change {
        BarChange::Add {
            name,
            progress,
            style,
        } => BossOperation::Add {
            name: name_nbt(name),
            progress,
            colour: i32::from(style.colour),
            overlay: i32::from(style.overlay),
            properties: properties(style),
        },
        BarChange::Remove => BossOperation::Remove,
        BarChange::Progress(progress) => BossOperation::UpdateProgress(progress),
        BarChange::Name(name) => BossOperation::UpdateName(name_nbt(name)),
    };
    Some(packets::encode(&BossEvent { id, operation }).into())
}

/// A bar's name as the game's component codec writes it into NBT: the dragon's is its
/// type's name.
fn name_nbt(name: BarName) -> Nbt {
    match name {
        BarName::Raid(name) => raid_name_nbt(name),
        BarName::Dragon => translatable("entity.minecraft.ender_dragon", None),
    }
}

/// A raid's bar's name as the game's component codec writes it into NBT.
fn raid_name_nbt(name: RaidBarName) -> Nbt {
    match name {
        RaidBarName::Raid => translatable("event.minecraft.raid", None),
        // `RAID_NAME_COMPONENT.copy().append(" - ").append(translatable(
        // "event.minecraft.raid.raiders_remaining", alive))`: the plain name with two
        // siblings, a string and a compound, which a list tag of mixed tags holds as
        // compounds, the string wrapped under the key "" (`ListTag.wrapIfNeeded`, as
        // `stack_wire.rs` writes a name's siblings).
        RaidBarName::RaidersRemaining(alive) => {
            let mut name = translatable("event.minecraft.raid", None);
            if let Nbt::Compound(entries) = &mut name {
                entries.push((
                    "extra".to_owned(),
                    Nbt::List(vec![
                        Nbt::Compound(vec![(String::new(), Nbt::String(" - ".to_owned()))]),
                        translatable("event.minecraft.raid.raiders_remaining", Some(alive)),
                    ]),
                ));
            }
            name
        }
        RaidBarName::Victory => translatable("event.minecraft.raid.victory.full", None),
        RaidBarName::Defeat => translatable("event.minecraft.raid.defeat.full", None),
    }
}

/// `{translate: key}`, with the one argument `arg` as an int tag where there is one
/// (`TranslatableContents`' arguments: a number is written as a number).
fn translatable(key: &str, arg: Option<i32>) -> Nbt {
    let mut entries = vec![("translate".to_owned(), Nbt::String(key.to_owned()))];
    if let Some(arg) = arg {
        entries.push(("with".to_owned(), Nbt::List(vec![Nbt::Int(arg)])));
    }
    Nbt::Compound(entries)
}

#[cfg(test)]
mod tests {
    use clustine_protocol::packet_ids::play::clientbound;
    use clustine_protocol::packets::play::ClientboundPlay;
    use clustine_protocol::text::Text;

    use super::*;

    const ID: Uuid = Uuid::from_u128(0x0123_4567_89ab_cdef_0011_2233_4455_6677);

    fn read(change: BarChange) -> BossEvent {
        let bytes = boss_bar(ID, change).expect("every change is sent");
        match ClientboundPlay::decode(&bytes).expect("a packet the edge writes") {
            ClientboundPlay::BossEvent(packet) => packet,
            other => panic!("not boss_event: {other:?}"),
        }
    }

    fn name_of(operation: &BossOperation) -> &Nbt {
        match operation {
            BossOperation::Add { name, .. } | BossOperation::UpdateName(name) => name,
            other => panic!("no name in {other:?}"),
        }
    }

    #[test]
    fn a_raid_s_bar_is_added_red_and_notched_in_ten_without_properties() {
        let bytes = boss_bar(
            ID,
            BarChange::Add {
                name: BarName::Raid(RaidBarName::Raid),
                progress: 0.0,
                style: BarStyle::RAID,
            },
        )
        .expect("sent");
        let mut expected = vec![u8::try_from(clientbound::BOSS_EVENT).expect("one byte")];
        expected.extend(ID.as_u128().to_be_bytes());
        // The operation (add), then the name: a compound of one string, `translate`.
        expected.push(0);
        expected.push(10);
        expected.push(8);
        expected.extend(9u16.to_be_bytes());
        expected.extend(b"translate");
        expected.extend(20u16.to_be_bytes());
        expected.extend(b"event.minecraft.raid");
        expected.push(0);
        // The progress, red, notched in ten, no properties.
        expected.extend(0.0f32.to_be_bytes());
        expected.extend([2, 2, 0]);
        assert_eq!(&bytes[..], &expected[..]);
    }

    /// The plan, section 19.5 item 7: the dragon's bar is pink, a plain bar, with the boss
    /// music and the fog, and named by the dragon's type.
    #[test]
    fn the_dragon_s_bar_is_added_pink_and_plain_with_music_and_fog() {
        let packet = read(BarChange::Add {
            name: BarName::Dragon,
            progress: 1.0,
            style: BarStyle::DRAGON,
        });
        let BossOperation::Add {
            colour,
            overlay,
            properties,
            ref name,
            ..
        } = packet.operation
        else {
            panic!("an addition: {packet:?}");
        };
        assert_eq!((colour, overlay, properties), (0, 0, 0b110));
        assert_eq!(
            Text::from_nbt(name.clone()),
            Text::translatable("entity.minecraft.ender_dragon")
        );
    }

    #[test]
    fn the_raid_s_bar_names_its_raiders_while_one_or_two_are_left() {
        let packet = read(BarChange::Name(BarName::Raid(
            RaidBarName::RaidersRemaining(2),
        )));
        assert_eq!(packet.id, ID);
        assert_eq!(
            name_of(&packet.operation),
            &Nbt::Compound(vec![
                (
                    "translate".into(),
                    Nbt::String("event.minecraft.raid".into())
                ),
                (
                    "extra".into(),
                    Nbt::List(vec![
                        Nbt::Compound(vec![(String::new(), Nbt::String(" - ".into()))]),
                        Nbt::Compound(vec![
                            (
                                "translate".into(),
                                Nbt::String("event.minecraft.raid.raiders_remaining".into()),
                            ),
                            ("with".into(), Nbt::List(vec![Nbt::Int(2)])),
                        ]),
                    ]),
                ),
            ])
        );
        assert_eq!(
            Text::from_nbt(name_of(&packet.operation).clone()).translation_key(),
            Some("event.minecraft.raid")
        );
    }

    #[test]
    fn victory_and_defeat_are_named_by_their_full_sentences() {
        for (name, key) in [
            (RaidBarName::Raid, "event.minecraft.raid"),
            (RaidBarName::Victory, "event.minecraft.raid.victory.full"),
            (RaidBarName::Defeat, "event.minecraft.raid.defeat.full"),
        ] {
            let packet = read(BarChange::Name(BarName::Raid(name)));
            assert_eq!(
                Text::from_nbt(name_of(&packet.operation).clone()),
                Text::translatable(key)
            );
        }
    }

    #[test]
    fn the_bar_s_progress_and_its_removal_go_as_their_operations() {
        assert_eq!(
            read(BarChange::Progress(0.375)).operation,
            BossOperation::UpdateProgress(0.375)
        );
        assert_eq!(read(BarChange::Remove).operation, BossOperation::Remove);
        let bytes = boss_bar(ID, BarChange::Remove).expect("sent");
        assert_eq!(bytes.len(), 1 + 16 + 1);
        assert_eq!(bytes[17], 1);
        let bytes = boss_bar(ID, BarChange::Progress(0.375)).expect("sent");
        assert_eq!(bytes[17], 2);
        assert_eq!(&bytes[18..], &0.375f32.to_be_bytes());
    }

    #[test]
    fn an_added_bar_carries_the_progress_it_was_given() {
        let packet = read(BarChange::Add {
            name: BarName::Raid(RaidBarName::Victory),
            progress: 1.0,
            style: BarStyle::RAID,
        });
        assert_eq!(
            packet.operation,
            BossOperation::Add {
                name: Text::translatable("event.minecraft.raid.victory.full").to_nbt(),
                progress: 1.0,
                colour: 2,
                overlay: 2,
                properties: 0,
            }
        );
    }
}
