//! Links between processes, over TCP.
//!
//! Whoever opens a connection that is about a region first says which region and whom
//! it takes for its owner, and is told whether that is accepted; see [`connect`] and
//! [`accept`]. After that the connection is a link like any other.

use std::io;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::link::{End, over_stream};
use crate::{RegionHello, RegionWelcome, wire};

/// How long the other side may take over its part of the greeting.
const GREETING_TIMEOUT: Duration = Duration::from_secs(10);

/// Why a connection about a region did not come about.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("the other side refused: {0}")]
    Refused(String),
    #[error("the other side did not answer")]
    Silent,
}

/// A link over an established connection. Must be called within a tokio runtime.
pub fn link<Out, In>(stream: TcpStream, capacity: usize) -> End<Out, In>
where
    Out: Serialize + Send + 'static,
    In: DeserializeOwned + Send + 'static,
{
    // Messages are written in batches already, and a tick must not wait for the next.
    let _ = stream.set_nodelay(true);
    over_stream(stream, capacity)
}

/// Connects to `address` about the region `hello` names and, if the other side accepts,
/// returns the link. Each direction queues up to `capacity` messages.
pub async fn connect<Out, In>(
    address: &str,
    hello: RegionHello,
    capacity: usize,
) -> Result<End<Out, In>, ConnectError>
where
    Out: Serialize + Send + 'static,
    In: DeserializeOwned + Send + 'static,
{
    let mut stream = TcpStream::connect(address).await?;
    wire::write(&mut stream, &hello).await?;
    stream.flush().await?;
    let welcome = timeout(GREETING_TIMEOUT, wire::read(&mut stream))
        .await
        .map_err(|_| ConnectError::Silent)??;
    match welcome {
        Some(RegionWelcome::Accepted) => Ok(link(stream, capacity)),
        Some(RegionWelcome::Refused { reason }) => Err(ConnectError::Refused(reason)),
        None => Err(ConnectError::Silent),
    }
}

/// A connection whose other side has said what it wants and waits for the answer.
#[derive(Debug)]
pub struct Incoming {
    stream: TcpStream,
    hello: RegionHello,
}

/// Reads what the other side of a connection that was just accepted wants.
pub async fn accept(mut stream: TcpStream) -> io::Result<Incoming> {
    let hello = timeout(GREETING_TIMEOUT, wire::read(&mut stream))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no greeting"))??
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no greeting"))?;
    Ok(Incoming { stream, hello })
}

impl Incoming {
    pub fn hello(&self) -> RegionHello {
        self.hello
    }

    /// Accepts the connection, which then is a link. Each direction queues up to
    /// `capacity` messages.
    pub async fn welcome<Out, In>(mut self, capacity: usize) -> io::Result<End<Out, In>>
    where
        Out: Serialize + Send + 'static,
        In: DeserializeOwned + Send + 'static,
    {
        wire::write(&mut self.stream, &RegionWelcome::Accepted).await?;
        self.stream.flush().await?;
        Ok(link(self.stream, capacity))
    }

    /// Tells the other side why it is not accepted and closes the connection.
    pub async fn refuse(mut self, reason: impl Into<String>) -> io::Result<()> {
        let refusal = RegionWelcome::Refused {
            reason: reason.into(),
        };
        wire::write(&mut self.stream, &refusal).await?;
        self.stream.shutdown().await
    }
}

#[cfg(test)]
mod tests {
    use clustine_region::RegionId;
    use clustine_world::{ChunkPos, PlayerId};
    use tokio::net::TcpListener;
    use uuid::Uuid;

    use super::*;
    use crate::link::{EdgeEnd, WorkerEnd};
    use crate::{EdgeMessage, EdgeToWorker, WorkerToEdge};

    fn hello() -> RegionHello {
        RegionHello {
            region: RegionId(1),
            epoch: 5,
            layout: 42,
        }
    }

    async fn listener() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        (listener, address)
    }

    #[tokio::test]
    async fn an_accepted_connection_is_a_link() {
        let (listener, address) = listener().await;
        let worker = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let incoming = accept(stream).await.unwrap();
            assert_eq!(incoming.hello(), hello());
            let link: WorkerEnd = incoming.welcome(64).await.unwrap();
            link
        });
        let mut edge: EdgeEnd = connect(&address, hello(), 64).await.unwrap();
        let mut worker = worker.await.unwrap();

        let chunks: Vec<_> = (0..1000).map(|x| ChunkPos::new(x, -x)).collect();
        // Many small messages and a large one, which the writing side batches.
        for round in 0..50 {
            let player = PlayerId(Uuid::from_u128(round));
            let leave = EdgeMessage {
                number: Some(round as u64 + 1),
                body: EdgeToWorker::PlayerLeave { player },
            };
            edge.send(leave).await.unwrap();
        }
        let subscribe = EdgeMessage::unnumbered(EdgeToWorker::Subscribe {
            ask: 1,
            chunks: chunks.clone(),
        });
        edge.send(subscribe.clone()).await.unwrap();
        for round in 0..50 {
            let player = PlayerId(Uuid::from_u128(round));
            let leave = EdgeMessage {
                number: Some(round as u64 + 1),
                body: EdgeToWorker::PlayerLeave { player },
            };
            assert_eq!(worker.recv().await, Some(leave));
        }
        assert_eq!(worker.recv().await, Some(subscribe));

        let delta = WorkerToEdge::TickDelta {
            tick: 9,
            events: Vec::new(),
        };
        worker.send(delta.clone()).await.unwrap();
        assert_eq!(edge.recv().await, Some(delta));

        // Either side notices the other going away.
        drop(edge);
        assert_eq!(worker.recv().await, None);
    }

    #[tokio::test]
    async fn a_refusal_carries_its_reason() {
        let (listener, address) = listener().await;
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let incoming = accept(stream).await.unwrap();
            incoming.refuse("that region is not here").await.unwrap();
        });
        let result: Result<EdgeEnd, _> = connect(&address, hello(), 8).await;
        assert!(
            matches!(&result, Err(ConnectError::Refused(reason)) if reason == "that region is not here"),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn a_listener_that_goes_away_is_reported() {
        let (listener, address) = listener().await;
        tokio::spawn(async move {
            // Accepted and dropped without a word.
            let _ = listener.accept().await.unwrap();
        });
        let result: Result<EdgeEnd, _> = connect(&address, hello(), 8).await;
        assert!(
            matches!(result, Err(ConnectError::Silent | ConnectError::Io(_))),
            "{result:?}"
        );
    }

    /// Neither side waits forever for a greeting that does not come.
    #[tokio::test(start_paused = true)]
    async fn silence_times_out() {
        let (listener, address) = listener().await;
        let silent_client = TcpStream::connect(&address).await.unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let error = accept(stream).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        drop(silent_client);

        // Now the listener is the one that says nothing.
        let connecting = tokio::spawn(async move {
            let result: Result<EdgeEnd, _> = connect(&address, hello(), 8).await;
            result
        });
        let (_held_open, _) = listener.accept().await.unwrap();
        assert!(matches!(
            connecting.await.unwrap(),
            Err(ConnectError::Silent)
        ));
    }
}
