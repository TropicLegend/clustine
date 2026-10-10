//! How messages travel over a byte stream: each as a 32-bit length followed by its
//! serialised form.
//!
//! The functions here do it for the asynchronous streams of tokio; [`blocking`] has the
//! same for the standard library's streams, for services that do without a runtime.

use std::io;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Messages larger than this are not accepted from a byte stream.
pub const MAX_MESSAGE_LENGTH: u32 = 16 * 1024 * 1024;

/// The bytes that go over the stream for `message`.
fn encode<T: Serialize>(message: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    encode_into(&mut bytes, message);
    bytes
}

/// Appends the bytes that go over the stream for `message`.
pub(crate) fn encode_into<T: Serialize>(bytes: &mut Vec<u8>, message: &T) {
    let start = bytes.len();
    // Room for the length, which is only known afterwards.
    bytes.extend_from_slice(&[0; 4]);
    *bytes =
        postcard::to_extend(message, std::mem::take(bytes)).expect("messages are serialisable");
    let length = u32::try_from(bytes.len() - start - 4).expect("a message fits a 32-bit length");
    bytes[start..start + 4].copy_from_slice(&length.to_be_bytes());
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    postcard::from_bytes(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn too_long(length: u32) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("a message of {length} bytes is too long"),
    )
}

/// Writes one message. The stream is not flushed.
pub async fn write<T: Serialize>(
    writer: &mut (impl AsyncWrite + Unpin),
    message: &T,
) -> io::Result<()> {
    writer.write_all(&encode(message)).await
}

/// Reads one message. Returns `None` if the stream ended where a message would begin.
pub async fn read<T: DeserializeOwned>(
    reader: &mut (impl AsyncRead + Unpin),
) -> io::Result<Option<T>> {
    let mut length = [0; 4];
    // A stream may end between two messages, but not within one.
    if reader.read(&mut length[..1]).await? == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut length[1..]).await?;
    let length = u32::from_be_bytes(length);
    if length > MAX_MESSAGE_LENGTH {
        return Err(too_long(length));
    }
    let mut bytes = vec![0; length as usize];
    reader.read_exact(&mut bytes).await?;
    decode(&bytes).map(Some)
}

/// The same for streams that block.
pub mod blocking {
    use std::io::{self, Read, Write};

    use serde::Serialize;
    use serde::de::DeserializeOwned;

    use super::{MAX_MESSAGE_LENGTH, decode, encode, too_long};

    /// Writes one message. The stream is not flushed.
    pub fn write<T: Serialize>(writer: &mut impl Write, message: &T) -> io::Result<()> {
        writer.write_all(&encode(message))
    }

    /// Reads one message. Returns `None` if the stream ended where a message would begin.
    pub fn read<T: DeserializeOwned>(reader: &mut impl Read) -> io::Result<Option<T>> {
        let mut length = [0; 4];
        loop {
            match reader.read(&mut length[..1]) {
                Ok(0) => return Ok(None),
                Ok(_) => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        reader.read_exact(&mut length[1..])?;
        let length = u32::from_be_bytes(length);
        if length > MAX_MESSAGE_LENGTH {
            return Err(too_long(length));
        }
        let mut bytes = vec![0; length as usize];
        reader.read_exact(&mut bytes)?;
        decode(&bytes).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::{RegionHello, RegionWelcome};
    use clustine_region::RegionId;

    fn hello() -> RegionHello {
        RegionHello {
            region: RegionId(3),
            epoch: 77,
        }
    }

    /// `message` comes out as it went in.
    fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(message: T) {
        // Without the length that goes before a message on a stream.
        let decoded: T = decode(&encode(&message)[4..]).unwrap();
        assert_eq!(decoded, message);
    }

    /// What one side writes, the other reads, whichever of the two kinds each side is.
    #[tokio::test]
    async fn both_kinds_of_stream_share_one_format() {
        let refusal = RegionWelcome::Refused {
            reason: "no".to_owned(),
        };

        let mut written = Vec::new();
        write(&mut written, &hello()).await.unwrap();
        write(&mut written, &refusal).await.unwrap();
        let mut blocking_written = Vec::new();
        blocking::write(&mut blocking_written, &hello()).unwrap();
        blocking::write(&mut blocking_written, &refusal).unwrap();
        assert_eq!(written, blocking_written);

        let mut reader = Cursor::new(&written);
        assert_eq!(blocking::read(&mut reader).unwrap(), Some(hello()));
        assert_eq!(blocking::read(&mut reader).unwrap(), Some(refusal.clone()));
        assert_eq!(blocking::read::<RegionHello>(&mut reader).unwrap(), None);

        let mut reader = &written[..];
        assert_eq!(read(&mut reader).await.unwrap(), Some(hello()));
        assert_eq!(read(&mut reader).await.unwrap(), Some(refusal));
        assert_eq!(read::<RegionHello>(&mut reader).await.unwrap(), None);
    }

    /// The messages of regions that hold chunks and merge and split, which nobody
    /// sends yet, come out as they went in.
    #[test]
    fn the_messages_of_regions_that_follow_players_round_trip() {
        use clustine_region::RegionId;
        use clustine_sim::api::{Durable, PlayerInput};
        use clustine_world::{ChunkArea, ChunkPos, EntityId, PlayerId};

        use crate::{
            ChunkBox, Decline, EdgeMessage, EdgeToWorker, FromCoordinator, Off, PlayersOf,
            RegionInfo, RegionList, SplitPart, StoreReply, StoreRequest, ToCoordinator, Welcome,
            WorkerToEdge,
        };

        let chunks = vec![ChunkPos::new(-3, 7), ChunkPos::new(4, -1)];
        let region = RegionId(9);
        let areas = vec![
            ChunkArea {
                min_x: Some(4),
                max_x: None,
            },
            ChunkArea {
                min_x: None,
                max_x: Some(-9),
            },
        ];
        let player = PlayerId(uuid::Uuid::from_u128(7));
        for request in [
            StoreRequest::Claim {
                chunks: chunks.clone(),
            },
            StoreRequest::Return {
                chunks: chunks.clone(),
            },
            StoreRequest::AbsorbCommit {
                absorbed: region,
                absorbed_epoch: 4,
                tick: 13,
                state: vec![1, 2],
            },
            StoreRequest::SplitCommit {
                tick: 14,
                state: vec![3],
                part: SplitPart {
                    chunks: chunks.clone(),
                    state: vec![4, 5],
                },
                as_epoch: 77,
                region: RegionId(10),
            },
        ] {
            round_trip(request);
        }
        for reply in [
            StoreReply::Claimed {
                granted: chunks[..1].to_vec(),
                foreign: vec![(chunks[1], region)],
            },
            StoreReply::Absorbed {
                absorbed: region,
                chunks: chunks.clone(),
                pinned: areas.clone(),
            },
            StoreReply::Absorbed {
                absorbed: region,
                chunks: Vec::new(),
                pinned: Vec::new(),
            },
            StoreReply::Split { region },
            StoreReply::Declined {
                reason: Decline::NotNext { next: region },
            },
            StoreReply::Declined {
                reason: Decline::Uncheckpointed { region },
            },
            StoreReply::Declined {
                reason: Decline::Tick { named: 12 },
            },
            StoreReply::Declined {
                reason: Decline::NotOpened { epoch: Some(3) },
            },
            StoreReply::Declined {
                reason: Decline::NotHeld { chunk: chunks[0] },
            },
            StoreReply::NotHeld {
                position: chunks[0],
                holder: Some(region),
            },
            StoreReply::NotHeld {
                position: chunks[1],
                holder: None,
            },
        ] {
            round_trip(reply);
        }
        round_trip(RegionList {
            home: RegionId(0),
            regions: vec![RegionInfo {
                region,
                epoch: 5,
                bounds: Some(ChunkBox {
                    min: chunks[0],
                    max: chunks[1],
                }),
                pinned: areas,
            }],
            absorbed: vec![(RegionId(3), region)],
            next: RegionId(10),
        });
        // What an edge says of a stay, with the entity that names it and without.
        for body in [
            EdgeToWorker::PlayerLeave {
                player,
                entity: Some(EntityId(41)),
                attempt: None,
            },
            EdgeToWorker::PlayerLeave {
                player,
                entity: None,
                attempt: None,
            },
            EdgeToWorker::Input {
                player,
                entity: EntityId(41),
                number: 6,
                input: PlayerInput::SelectSlot { slot: 3 },
            },
        ] {
            assert!(body.is_numbered());
            round_trip(EdgeMessage {
                number: Some(12),
                body,
            });
        }
        // The welcomes that count their presence answers, and the two entries a merge
        // and a split leave in an outbox.
        for said in [
            WorkerToEdge::Welcome(Welcome::Resumed {
                entries: 2,
                presences: 3,
                applied: 17,
            }),
            WorkerToEdge::Welcome(Welcome::Unknown {
                since: 40,
                entries: 0,
                presences: u32::MAX,
                applied: 0,
            }),
            WorkerToEdge::Welcome(Welcome::Unknown {
                since: 41,
                entries: 1,
                presences: 0,
                applied: u64::MAX,
            }),
            WorkerToEdge::Welcome(Welcome::Superseded),
            WorkerToEdge::Outbox {
                number: 5,
                entry: Durable::Absorbed {
                    region,
                    since: 31,
                    applied: 17,
                    numbers: vec![2, 3, 8],
                },
            },
            WorkerToEdge::Outbox {
                number: 6,
                entry: Durable::Absorbed {
                    region,
                    since: 0,
                    applied: 0,
                    numbers: Vec::new(),
                },
            },
            WorkerToEdge::Outbox {
                number: 7,
                entry: Durable::SplitOff {
                    region: RegionId(10),
                    players: vec![
                        (player, EntityId(41), Some(3)),
                        (player, EntityId(42), None),
                    ],
                },
            },
        ] {
            round_trip(said);
        }
        round_trip(EdgeToWorker::SubscribeAsGuest {
            ask: 7,
            chunks: chunks.clone(),
        });
        round_trip(WorkerToEdge::Elsewhere {
            chunk: chunks[0],
            ask: 7,
            region,
        });
        round_trip(WorkerToEdge::NotMine {
            chunk: chunks[1],
            ask: 8,
        });
        for said in [
            ToCoordinator::Players {
                regions: vec![PlayersOf {
                    region,
                    epoch: 5,
                    tick: 1_200,
                    crowds: vec![(chunks[0], 3)],
                }],
            },
            ToCoordinator::Merge {
                survivor: RegionId(0),
                absorbed: region,
            },
            ToCoordinator::Split {
                region,
                chunks: chunks.clone(),
            },
            ToCoordinator::AbsorbEnded {
                region: RegionId(0),
                absorbed: region,
                outcome: Ok(()),
            },
            ToCoordinator::SplitEnded {
                region,
                as_epoch: 6,
                outcome: Ok(RegionId(10)),
            },
        ] {
            round_trip(said);
        }
        // Every reason a merge or a split can come to nothing for, as a worker says it
        // of either.
        for why in [
            Off::NotRunning,
            Off::Busy,
            Off::Nobody,
            Off::NothingStays,
            Off::TooLarge,
            Off::Declined(Decline::NotNext { next: RegionId(10) }),
            Off::Declined(Decline::NotOpened { epoch: None }),
            Off::StoreLost,
            Off::Unreadable,
            Off::Refused,
        ] {
            round_trip(ToCoordinator::AbsorbEnded {
                region: RegionId(0),
                absorbed: region,
                outcome: Err(why),
            });
            round_trip(ToCoordinator::SplitEnded {
                region,
                as_epoch: 6,
                outcome: Err(why),
            });
        }
        for said in [
            FromCoordinator::Absorb {
                region: RegionId(0),
                epoch: 4,
                absorbed: region,
                as_epoch: 5,
            },
            FromCoordinator::SplitOff {
                region,
                epoch: 4,
                chunks,
                as_epoch: 6,
                part: RegionId(10),
            },
            FromCoordinator::Prepare {
                region: RegionId(0),
                epoch: 4,
            },
            FromCoordinator::Asked(Ok(region)),
            FromCoordinator::Asked(Err("no such region".to_owned())),
        ] {
            round_trip(said);
        }
    }

    /// Where a worker says its players are comes out as it went in: of no region, of
    /// a region without players, and of regions with them, in the order they were
    /// named and with numbers at the ends of what they can be.
    #[test]
    fn where_a_worker_says_its_players_are_round_trips() {
        use clustine_world::ChunkPos;

        use crate::{PlayersOf, ToCoordinator};

        round_trip(ToCoordinator::Players {
            regions: Vec::new(),
        });
        let empty = PlayersOf {
            region: RegionId(0),
            epoch: 0,
            tick: 0,
            crowds: Vec::new(),
        };
        let crowded = PlayersOf {
            region: RegionId(u32::MAX),
            epoch: u64::MAX,
            tick: u64::MAX,
            crowds: vec![
                (ChunkPos::new(i32::MIN, i32::MAX), 1),
                (ChunkPos::new(-1, 0), u32::MAX),
                (ChunkPos::new(i32::MAX, i32::MIN), 0),
            ],
        };
        round_trip(empty.clone());
        round_trip(crowded.clone());
        round_trip(ToCoordinator::Players {
            regions: vec![crowded.clone(), empty, crowded],
        });
    }

    /// What the world store answers a hello and a commit with comes out as it went in.
    #[test]
    fn the_messages_of_the_world_store_round_trip() {
        use clustine_data::blocks;
        use clustine_world::{BlockPos, ChunkArea, ChunkPos, EntityId, EntityIds};

        use crate::{
            Restored, RestoredItem, RestoredPart, RestoredPiece, StoreReply, StoreRequest,
            StoreWelcome, TickState,
        };

        let restored = Restored {
            held: Vec::new(),
            pinned: vec![ChunkArea {
                min_x: None,
                max_x: Some(0),
            }],
            entity_ids: EntityIds::block(3).unwrap(),
            state: Some(TickState {
                tick: 7,
                state: vec![1, 2, 3],
            }),
            deltas: vec![
                TickState {
                    tick: 8,
                    state: Vec::new(),
                },
                TickState {
                    tick: 9,
                    state: vec![0xFF; 300],
                },
            ],
            issued: EntityId(0),
        };
        assert_eq!(restored.tick(), 9);
        let welcomes = [
            StoreWelcome::Accepted {
                entity_ids: restored.entity_ids,
                pinned: restored.pinned.clone(),
                issued: EntityId(i32::MAX),
            },
            StoreWelcome::EpochRefused { seen: u64::MAX },
            StoreWelcome::Absorbed {
                into: clustine_region::RegionId(8),
            },
            StoreWelcome::Refused {
                reason: "no".to_owned(),
            },
        ];
        let mut written = Vec::new();
        for welcome in &welcomes {
            blocking::write(&mut written, welcome).unwrap();
        }
        let request = StoreRequest::Commit {
            tick: 9,
            changes: vec![(BlockPos::new(-1, -64, 3), blocks::GLASS)],
            state: vec![4, 5],
            stays: Vec::new(),
        };
        blocking::write(&mut written, &request).unwrap();
        blocking::write(&mut written, &StoreReply::Committed { tick: 9 }).unwrap();

        let mut reader = Cursor::new(&written);
        for welcome in welcomes {
            assert_eq!(blocking::read(&mut reader).unwrap(), Some(welcome));
        }
        assert_eq!(blocking::read(&mut reader).unwrap(), Some(request));
        let reply = blocking::read(&mut reader).unwrap();
        assert_eq!(reply, Some(StoreReply::Committed { tick: 9 }));

        // The parts a welcome is followed by: one that ends in the middle of a delta,
        // and one with nothing in it but that it is the last.
        let parts = [
            RestoredPart {
                pieces: vec![
                    RestoredPiece {
                        of: RestoredItem::State,
                        tick: 7,
                        bytes: vec![1, 2, 3],
                        complete: true,
                    },
                    RestoredPiece {
                        of: RestoredItem::Delta,
                        tick: u64::MAX,
                        bytes: vec![0xFF; 300],
                        complete: false,
                    },
                ],
                last: false,
            },
            RestoredPart {
                pieces: Vec::new(),
                last: true,
            },
        ];
        let mut written = Vec::new();
        for part in &parts {
            blocking::write(&mut written, part).unwrap();
        }
        let mut reader = Cursor::new(&written);
        for part in parts {
            assert_eq!(blocking::read(&mut reader).unwrap(), Some(part));
        }

        // The grants of a region travel as the bytes of a piece.
        let held = vec![(ChunkPos::new(-3, 7), 0), (ChunkPos::new(4, -1), u64::MAX)];
        let piece = RestoredPiece {
            of: RestoredItem::Held,
            tick: 0,
            bytes: crate::held_bytes(&held),
            complete: true,
        };
        let mut written = Vec::new();
        blocking::write(&mut written, &piece).unwrap();
        let read: RestoredPiece = blocking::read(&mut Cursor::new(&written)).unwrap().unwrap();
        assert_eq!(crate::held_from_bytes(&read.bytes), Some(held));
        assert_eq!(
            crate::held_from_bytes(&crate::held_bytes(&[])),
            Some(Vec::new())
        );
        assert_eq!(crate::held_from_bytes(&[0xFF; 3]), None);

        // A region that has never committed anything is restored up to tick 0, and one
        // with a state and no commits after it up to the state's tick.
        let mut fresh = Restored {
            held: Vec::new(),
            pinned: Vec::new(),
            entity_ids: EntityIds::block(0).unwrap(),
            state: None,
            deltas: Vec::new(),
            issued: EntityId(0),
        };
        assert_eq!(fresh.tick(), 0);
        fresh.state = Some(TickState {
            tick: 4,
            state: Vec::new(),
        });
        assert_eq!(fresh.tick(), 4);
    }

    /// What a region and the world store say to each other about stays, and what the
    /// store says of the entity ids that were given out, come out as they went in. See
    /// `docs/adr/0020-one-stay-per-player.md`, sections 3, 5 and 7.
    #[test]
    fn what_is_said_of_stays_to_and_by_the_world_store_round_trips() {
        use clustine_sim::api::{HOTBAR_SLOTS, ItemStack, Place, Pose, StayNote};
        use clustine_world::{EntityId, EntityIds, PlayerId, Vec3};

        use crate::{Restored, StoreReply, StoreRequest, StoreWelcome};

        let player = PlayerId(uuid::Uuid::from_u128(7));
        let other = PlayerId(uuid::Uuid::from_u128(u128::MAX));
        let mut hotbar = [None; HOTBAR_SLOTS];
        hotbar[0] = Some(ItemStack { item: 1, count: 64 });
        hotbar[8] = Some(ItemStack { item: 35, count: 1 });
        let place = Place {
            pose: Pose {
                position: Vec3::new(-1234.5, 71.25, 0.5),
                yaw: -179.5,
                pitch: 89.0,
                on_ground: false,
            },
            flying: true,
            hotbar,
            selected_slot: 8,
        };
        let standing = Place {
            pose: Pose::at(Vec3::new(8.5, -60.0, 8.5)),
            flying: false,
            hotbar: [None; HOTBAR_SLOTS],
            selected_slot: 0,
        };

        round_trip(StoreRequest::Commit {
            tick: 9,
            changes: Vec::new(),
            state: vec![4, 5],
            stays: vec![
                StayNote::Entering {
                    player,
                    entity: EntityId(41),
                },
                StayNote::Has {
                    player,
                    entity: EntityId(41),
                    hops: 0,
                    place: standing.clone(),
                },
                StayNote::Has {
                    player: other,
                    entity: EntityId(i32::MAX),
                    hops: u32::MAX,
                    place: place.clone(),
                },
            ],
        });
        for reply in [
            StoreReply::Enter {
                player,
                entity: EntityId(41),
                place: None,
                holder: None,
            },
            StoreReply::Enter {
                player,
                entity: EntityId(41),
                place: Some(standing),
                holder: None,
            },
            StoreReply::Enter {
                player: other,
                entity: EntityId(i32::MAX),
                place: Some(place),
                holder: Some(clustine_region::RegionId(u32::MAX)),
            },
            StoreReply::Dead { stays: Vec::new() },
            StoreReply::Dead {
                stays: vec![(player, EntityId(41), 0), (other, EntityId(42), u32::MAX)],
            },
        ] {
            round_trip(reply);
        }
        for issued in [EntityId(0), EntityId(41), EntityId(i32::MAX)] {
            round_trip(StoreWelcome::Accepted {
                entity_ids: EntityIds::block(3).unwrap(),
                pinned: Vec::new(),
                issued,
            });
            round_trip(Restored {
                held: Vec::new(),
                pinned: Vec::new(),
                entity_ids: EntityIds::block(3).unwrap(),
                state: None,
                deltas: Vec::new(),
                issued,
            });
        }
    }

    /// What is said first to the world store is one of two things, and neither is
    /// taken for the other, nor a bare hello for either.
    #[test]
    fn what_is_said_first_to_the_world_store_round_trips() {
        use crate::StoreHello;

        let said = [StoreHello::Region(hello()), StoreHello::Regions];
        let mut written = Vec::new();
        for hello in &said {
            blocking::write(&mut written, hello).unwrap();
        }
        let mut reader = Cursor::new(&written);
        for hello in said {
            assert_eq!(blocking::read(&mut reader).unwrap(), Some(hello));
        }
        assert_eq!(blocking::read::<StoreHello>(&mut reader).unwrap(), None);

        // A hello for a region is the hello behind a byte that says which of the two
        // it is.
        let bare = encode(&hello());
        let wrapped = encode(&StoreHello::Region(hello()));
        assert_eq!(wrapped[4], 0);
        assert_eq!(wrapped[5..], bare[4..]);
        assert_eq!(encode(&StoreHello::Regions)[4..], [1]);
    }

    #[tokio::test]
    async fn a_stream_that_ends_within_a_message_is_an_error() {
        let mut written = Vec::new();
        write(&mut written, &hello()).await.unwrap();
        for cut in 1..written.len() {
            let error = read::<RegionHello>(&mut &written[..cut]).await.unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof, "cut at {cut}");
            let error = blocking::read::<RegionHello>(&mut &written[..cut]).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof, "cut at {cut}");
        }
    }

    #[tokio::test]
    async fn oversized_and_malformed_messages_are_errors() {
        let oversized = (MAX_MESSAGE_LENGTH + 1).to_be_bytes();
        let error = read::<RegionHello>(&mut &oversized[..]).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let error = blocking::read::<RegionHello>(&mut &oversized[..]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        // A length that is right, followed by something that is no welcome.
        let malformed = [0, 0, 0, 1, 9];
        let error = read::<RegionWelcome>(&mut &malformed[..])
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
