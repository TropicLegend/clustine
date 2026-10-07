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

/// How long to wait for a client to hang up after it was told to disconnect.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

/// Why a connection was ended by the edge.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ConnectionError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("the client took too long to send what was expected or to take what was sent")]
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
    /// How long the client may take to take what is sent to it.
    timeout: Duration,
    /// Whether the same limit applies to waiting for the client to send something.
    reads_are_timed: bool,
}

impl Connection {
    pub(crate) fn new(stream: TcpStream, timeout: Duration) -> Self {
        Self {
            stream,
            decoder: FrameDecoder::new(),
            encoder: FrameEncoder::new(),
            outgoing: Vec::new(),
            timeout,
            reads_are_timed: true,
        }
    }

    /// From now on, waits for the client to send something for as long as it takes.
    ///
    /// In the play state a client may have nothing to say for a while; whether it is
    /// still there is found out with keep-alives instead.
    pub(crate) fn stop_timing_reads(&mut self) {
        self.reads_are_timed = false;
    }

    /// Waits for the first byte the client sends and returns it without consuming it.
    /// `None` if the client closed the connection without sending anything.
    pub(crate) async fn first_byte(&mut self) -> Result<Option<u8>, ConnectionError> {
        let mut first = [0];
        let peeked = timeout(self.timeout, self.stream.peek(&mut first))
            .await
            .map_err(|_| ConnectionError::TimedOut)??;
        Ok((peeked > 0).then_some(first[0]))
    }

    /// Switches both directions to the compressed packet format, in which packets of at
    /// least `threshold` bytes are compressed. The client has to have been told.
    pub(crate) fn enable_compression(&mut self, threshold: usize) {
        self.encoder.enable_compression(threshold);
        self.decoder.enable_compression();
    }

    /// Waits for the next packet. Returns `None` if the client closed the connection
    /// between two packets.
    pub(crate) async fn read_frame(&mut self) -> Result<Option<Bytes>, ConnectionError> {
        loop {
            if let Some(frame) = self.decoder.next_frame()? {
                return Ok(Some(frame));
            }
            let read = self.stream.read_buf(self.decoder.buffer());
            let read = if self.reads_are_timed {
                timeout(self.timeout, read)
                    .await
                    .map_err(|_| ConnectionError::TimedOut)??
            } else {
                read.await?
            };
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

    /// Sends everything that was queued. A client that does not take it in time, because
    /// it has stopped reading, is given up on.
    pub(crate) async fn flush(&mut self) -> Result<(), ConnectionError> {
        let result = timeout(self.timeout, self.stream.write_all(&self.outgoing)).await;
        self.outgoing.clear();
        result.map_err(|_| ConnectionError::TimedOut)??;
        Ok(())
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

#[cfg(test)]
mod tests {
    use clustine_protocol::packets::status::PongResponse;
    use tokio::net::TcpListener;

    use super::*;

    /// A client that stops reading must not hold up its connection for ever.
    #[tokio::test]
    async fn writing_to_a_client_that_does_not_read_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        // Connected, but never read from.
        let _client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let mut connection = Connection::new(stream, Duration::from_millis(200));

        // Far more than the operating system buffers between the two ends.
        let packet = packets::encode(&PongResponse { payload: 0 });
        for _ in 0..4_000_000 {
            connection.queue_encoded(&packet).unwrap();
        }
        assert!(matches!(
            connection.flush().await,
            Err(ConnectionError::TimedOut)
        ));
    }

    #[tokio::test]
    async fn a_client_that_reads_gets_everything() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let mut connection = Connection::new(stream, Duration::from_secs(5));

        let reader = tokio::spawn(async move {
            let mut received = Vec::new();
            client.read_to_end(&mut received).await.unwrap();
            received.len()
        });
        let packet = packets::encode(&PongResponse { payload: 0 });
        for _ in 0..100_000 {
            connection.queue_encoded(&packet).unwrap();
        }
        connection.flush().await.unwrap();
        drop(connection);
        // Each pong is a length byte, an id byte and eight bytes of payload.
        assert_eq!(reader.await.unwrap(), 100_000 * 10);
    }
}
