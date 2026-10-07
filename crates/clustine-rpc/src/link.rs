//! Connections between services.
//!
//! A link has two [`End`]s. Each sends one kind of message and receives the other, in
//! order and without loss, through bounded queues. When one end is dropped, the other
//! end notices.
//!
//! [`in_process`] connects two ends directly. [`framed`] sends every message through
//! serialisation and a byte stream first, as a link between two processes does.

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::{EdgeToWorker, WorkerToEdge};

/// The edge's end of its link to a worker.
pub type EdgeEnd = End<EdgeToWorker, WorkerToEdge>;
/// The worker's end of its link to an edge.
pub type WorkerEnd = End<WorkerToEdge, EdgeToWorker>;

/// Messages larger than this are not accepted from a byte stream.
const MAX_MESSAGE_LENGTH: u32 = 16 * 1024 * 1024;

/// Why a message could not be sent or received.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error("the other end of the link is gone")]
    Closed,
    #[error("the link's queue is full")]
    Full,
}

/// One end of a link: sends `Out` messages and receives `In` messages.
#[derive(Debug)]
pub struct End<Out, In> {
    sender: Sender<Out>,
    receiver: mpsc::Receiver<In>,
}

/// The sending half of an [`End`]. It can be cloned to send from several tasks.
#[derive(Debug)]
pub struct Sender<Out>(mpsc::Sender<Out>);

impl<Out> Clone for Sender<Out> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<Out> Sender<Out> {
    /// Sends a message, waiting while the queue is full.
    pub async fn send(&self, message: Out) -> Result<(), LinkError> {
        self.0.send(message).await.map_err(|_| LinkError::Closed)
    }

    /// Sends a message without waiting. A simulation thread uses this so that a slow
    /// receiver can never delay a tick.
    pub fn try_send(&self, message: Out) -> Result<(), LinkError> {
        self.0.try_send(message).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => LinkError::Full,
            mpsc::error::TrySendError::Closed(_) => LinkError::Closed,
        })
    }
}

impl<Out, In> End<Out, In> {
    /// A handle for sending that can be cloned.
    pub fn sender(&self) -> Sender<Out> {
        self.sender.clone()
    }

    /// See [`Sender::send`].
    pub async fn send(&self, message: Out) -> Result<(), LinkError> {
        self.sender.send(message).await
    }

    /// See [`Sender::try_send`].
    pub fn try_send(&self, message: Out) -> Result<(), LinkError> {
        self.sender.try_send(message)
    }

    /// Waits for the next message. Returns `None` once the other end is gone and
    /// everything it sent has been received.
    pub async fn recv(&mut self) -> Option<In> {
        self.receiver.recv().await
    }

    /// Returns the next message if one is waiting, without blocking.
    pub fn try_recv(&mut self) -> Result<Option<In>, LinkError> {
        match self.receiver.try_recv() {
            Ok(message) => Ok(Some(message)),
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => Err(LinkError::Closed),
        }
    }
}

/// A link within one process. Each direction queues up to `capacity` messages.
pub fn in_process<A, B>(capacity: usize) -> (End<A, B>, End<B, A>) {
    let (a_sender, a_receiver) = mpsc::channel(capacity);
    let (b_sender, b_receiver) = mpsc::channel(capacity);
    (
        End {
            sender: Sender(a_sender),
            receiver: b_receiver,
        },
        End {
            sender: Sender(b_sender),
            receiver: a_receiver,
        },
    )
}

/// A link whose messages are serialised and sent through an in-memory byte stream, the
/// way a link between two processes works. Must be called within a tokio runtime.
pub fn framed<A, B>(capacity: usize) -> (End<A, B>, End<B, A>)
where
    A: Serialize + DeserializeOwned + Send + 'static,
    B: Serialize + DeserializeOwned + Send + 'static,
{
    let (near, far) = tokio::io::duplex(64 * 1024);
    (over_stream(near, capacity), over_stream(far, capacity))
}

/// An end that exchanges its messages over `stream`.
fn over_stream<Out, In>(
    stream: impl AsyncRead + AsyncWrite + Send + 'static,
    capacity: usize,
) -> End<Out, In>
where
    Out: Serialize + Send + 'static,
    In: DeserializeOwned + Send + 'static,
{
    let (reader, writer) = tokio::io::split(stream);
    let (out_sender, out_receiver) = mpsc::channel(capacity);
    let (in_sender, in_receiver) = mpsc::channel(capacity);
    tokio::spawn(write_messages(out_receiver, writer));
    tokio::spawn(read_messages(reader, in_sender));
    End {
        sender: Sender(out_sender),
        receiver: in_receiver,
    }
}

/// Writes each message as a 32-bit length followed by its serialised form, until the
/// sending side or the stream is closed.
async fn write_messages<T: Serialize>(
    mut messages: mpsc::Receiver<T>,
    mut writer: impl AsyncWrite + Unpin,
) {
    while let Some(message) = messages.recv().await {
        let bytes = postcard::to_allocvec(&message).expect("link messages are serialisable");
        let length = u32::try_from(bytes.len()).expect("link message fits a 32-bit length");
        if writer.write_u32(length).await.is_err() || writer.write_all(&bytes).await.is_err() {
            break;
        }
    }
    // Tells the reading side that nothing more will come.
    let _ = writer.shutdown().await;
}

/// Reads messages written by [`write_messages`] until the stream ends, a message is
/// malformed, or the receiving side is gone.
async fn read_messages<T: DeserializeOwned>(
    mut reader: impl AsyncRead + Unpin,
    messages: mpsc::Sender<T>,
) {
    while let Ok(length) = reader.read_u32().await {
        if length > MAX_MESSAGE_LENGTH {
            break;
        }
        let mut bytes = vec![0; length as usize];
        if reader.read_exact(&mut bytes).await.is_err() {
            break;
        }
        let Ok(message) = postcard::from_bytes(&bytes) else {
            break;
        };
        if messages.send(message).await.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::{DIMENSION_TYPES, blocks};
    use clustine_sim::api::{
        EntityKind, EntityState, PlayerEvent, PlayerInput, PlayerJoin, Pose, RegionEvent,
    };
    use clustine_world::{Biome, Chunk, ChunkPos, EntityId, PlayerId, Vec3};
    use uuid::Uuid;

    use super::*;

    fn links() -> [(EdgeEnd, WorkerEnd); 2] {
        [in_process(8), framed(8)]
    }

    fn player() -> PlayerId {
        PlayerId(Uuid::from_u128(7))
    }

    fn chunk() -> Chunk {
        let overworld = DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == "minecraft:overworld")
            .unwrap();
        let mut chunk = Chunk::empty(overworld, Biome(3));
        chunk.set(1, -64, 2, blocks::BEDROCK);
        chunk.set(15, 319, 15, blocks::STONE);
        chunk
    }

    #[tokio::test]
    async fn every_message_survives_both_kinds_of_link() {
        let to_worker = vec![
            EdgeToWorker::PlayerJoin(PlayerJoin {
                player: player(),
                name: "Notch".to_owned(),
            }),
            EdgeToWorker::Subscribe {
                chunks: vec![ChunkPos::new(-1, 2), ChunkPos::new(3, -4)],
            },
            EdgeToWorker::Unsubscribe {
                chunks: vec![ChunkPos::new(-1, 2)],
            },
            EdgeToWorker::Input {
                player: player(),
                input: PlayerInput::Move {
                    position: Some(Vec3::new(1.5, -60.0, 2.5)),
                    rotation: Some((90.0, 10.0)),
                    on_ground: true,
                },
            },
            EdgeToWorker::PlayerLeave { player: player() },
        ];
        let to_edge = vec![
            WorkerToEdge::TickDelta {
                tick: 3,
                events: vec![RegionEvent::EntityMoved {
                    entity: EntityId(5),
                    pose: Pose::at(Vec3::new(1.5, -60.0, 2.5)),
                    previous_chunk: ChunkPos::new(0, 0),
                }],
            },
            WorkerToEdge::ToPlayer {
                player: player(),
                event: PlayerEvent::Spawned {
                    entity_id: EntityId(5),
                    position: Vec3::new(0.5, -60.0, 0.5),
                },
            },
            WorkerToEdge::ChunkSnapshot {
                position: ChunkPos::new(3, -4),
                tick: 99,
                chunk: chunk(),
                entities: vec![EntityState {
                    entity: EntityId(5),
                    kind: EntityKind::Player {
                        player: player(),
                        name: "Notch".to_owned(),
                    },
                    pose: Pose::at(Vec3::new(50.0, -60.0, -60.0)),
                }],
            },
            WorkerToEdge::TickDelta {
                tick: 100,
                events: vec![RegionEvent::EntityRemoved {
                    entity: EntityId(5),
                    chunk: ChunkPos::new(3, -4),
                }],
            },
        ];

        for (mut edge, mut worker) in links() {
            for message in &to_worker {
                edge.send(message.clone()).await.unwrap();
            }
            for message in &to_worker {
                assert_eq!(worker.recv().await.as_ref(), Some(message));
            }
            for message in &to_edge {
                worker.send(message.clone()).await.unwrap();
            }
            for message in &to_edge {
                assert_eq!(edge.recv().await.as_ref(), Some(message));
            }
        }
    }

    #[tokio::test]
    async fn dropping_one_end_closes_the_other() {
        for (edge, mut worker) in links() {
            edge.send(EdgeToWorker::PlayerLeave { player: player() })
                .await
                .unwrap();
            drop(edge);
            // What was sent before still arrives.
            assert!(worker.recv().await.is_some());
            assert_eq!(worker.recv().await, None);
        }
        for (mut edge, worker) in links() {
            drop(worker);
            assert_eq!(edge.recv().await, None);
        }
    }

    #[tokio::test]
    async fn try_recv_distinguishes_empty_from_closed() {
        let (edge, mut worker) = in_process::<EdgeToWorker, WorkerToEdge>(8);
        assert_eq!(worker.try_recv(), Ok(None));
        edge.try_send(EdgeToWorker::PlayerLeave { player: player() })
            .unwrap();
        assert!(matches!(worker.try_recv(), Ok(Some(_))));
        drop(edge);
        assert_eq!(worker.try_recv(), Err(LinkError::Closed));
    }

    #[tokio::test]
    async fn try_send_reports_a_full_queue_instead_of_waiting() {
        let (edge, _worker) = in_process::<EdgeToWorker, WorkerToEdge>(1);
        let message = EdgeToWorker::PlayerLeave { player: player() };
        assert_eq!(edge.try_send(message.clone()), Ok(()));
        assert_eq!(edge.try_send(message), Err(LinkError::Full));
    }

    #[tokio::test]
    async fn senders_can_be_cloned() {
        let (edge, mut worker) = in_process::<EdgeToWorker, WorkerToEdge>(8);
        let first = edge.sender();
        let second = first.clone();
        first
            .send(EdgeToWorker::PlayerLeave { player: player() })
            .await
            .unwrap();
        second
            .send(EdgeToWorker::PlayerLeave { player: player() })
            .await
            .unwrap();
        assert!(worker.recv().await.is_some());
        assert!(worker.recv().await.is_some());
    }
}
