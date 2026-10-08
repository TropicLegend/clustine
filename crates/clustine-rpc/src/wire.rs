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
            layout: u64::MAX,
        }
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

    /// What the world store answers a hello and a commit with comes out as it went in.
    #[test]
    fn the_messages_of_the_world_store_round_trip() {
        use clustine_data::blocks;
        use clustine_world::{BlockPos, EntityIds};

        use crate::{Restored, StoreReply, StoreRequest, StoreWelcome, TickState};

        let restored = Restored {
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
        };
        assert_eq!(restored.tick(), 9);
        let welcomes = [
            StoreWelcome::Accepted(restored),
            StoreWelcome::EpochRefused { seen: u64::MAX },
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

        // A region that has never committed anything is restored up to tick 0, and one
        // with a state and no commits after it up to the state's tick.
        let mut fresh = Restored {
            entity_ids: EntityIds::block(0).unwrap(),
            state: None,
            deltas: Vec::new(),
        };
        assert_eq!(fresh.tick(), 0);
        fresh.state = Some(TickState {
            tick: 4,
            state: Vec::new(),
        });
        assert_eq!(fresh.tick(), 4);
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
