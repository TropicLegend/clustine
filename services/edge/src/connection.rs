//! A client connection as a stream of packets.

use std::io;
use std::time::Duration;

use bytes::Bytes;
use clustine_protocol::codec::DecodeError;
use clustine_protocol::frame::{FrameDecoder, FrameEncoder, FrameError};
use clustine_protocol::packets::{self, Packet};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// How long a client may stay silent before it is dropped, as in vanilla.
pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// How long to wait for a client to hang up after it was told to disconnect.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

/// Why a connection was ended by the edge.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ConnectionError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("no data received for {} seconds", READ_TIMEOUT.as_secs())]
    TimedOut,
    #[error("connection closed in the middle of a packet")]
    UnexpectedEof,
    #[error("framing error: {0}")]
    Frame(#[from] FrameError),
    #[error("malformed packet: {0}")]
    Decode(#[from] DecodeError),
    #[error("protocol violation: {0}")]
    Protocol(&'static str),
    #[error("the server is shutting down")]
    ShuttingDown,
}

pub(crate) struct Connection {
    stream: TcpStream,
    decoder: FrameDecoder,
    encoder: FrameEncoder,
    /// Framed packets that have been queued but not sent.
    outgoing: Vec<u8>,
}

impl Connection {
    pub(crate) fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            decoder: FrameDecoder::new(),
            encoder: FrameEncoder::new(),
            outgoing: Vec::new(),
        }
    }

    /// Waits for the next packet. Returns `None` if the client closed the connection
    /// between two packets.
    pub(crate) async fn read_frame(&mut self) -> Result<Option<Bytes>, ConnectionError> {
        loop {
            if let Some(frame) = self.decoder.next_frame()? {
                return Ok(Some(frame));
            }
            let read = timeout(READ_TIMEOUT, self.stream.read_buf(self.decoder.buffer()))
                .await
                .map_err(|_| ConnectionError::TimedOut)??;
            if read == 0 {
                return if self.decoder.buffer().is_empty() {
                    Ok(None)
                } else {
                    Err(ConnectionError::UnexpectedEof)
                };
            }
        }
    }

    /// Adds `packet` to the output without sending it yet; see [`Connection::flush`].
    pub(crate) fn queue<P: Packet>(&mut self, packet: &P) -> Result<(), ConnectionError> {
        self.queue_encoded(&packets::encode(packet))
    }

    /// Like [`Connection::queue`], for a packet that is already encoded (id and body).
    pub(crate) fn queue_encoded(&mut self, packet: &[u8]) -> Result<(), ConnectionError> {
        self.encoder.encode(packet, &mut self.outgoing)?;
        Ok(())
    }

    /// Sends everything that was queued.
    pub(crate) async fn flush(&mut self) -> Result<(), ConnectionError> {
        let result = self.stream.write_all(&self.outgoing).await;
        self.outgoing.clear();
        Ok(result?)
    }

    /// Queues `packet` and sends it at once.
    pub(crate) async fn write<P: Packet>(&mut self, packet: &P) -> Result<(), ConnectionError> {
        self.queue(packet)?;
        self.flush().await
    }

    /// Closes the sending side and waits briefly for the client to close its side.
    ///
    /// Closing a socket that still has unread data makes the operating system reset the
    /// connection, and a reset can make the client discard a disconnect message it has
    /// not shown yet. Reading until the client hangs up avoids that.
    pub(crate) async fn close_gracefully(&mut self) {
        let _ = self.stream.shutdown().await;
        let drain = async {
            let mut discard = [0; 1024];
            while matches!(self.stream.read(&mut discard).await, Ok(read) if read > 0) {}
        };
        let _ = timeout(CLOSE_TIMEOUT, drain).await;
    }
}
