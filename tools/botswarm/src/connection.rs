//! A bot's connection to a server as a stream of packets.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use bytes::Bytes;
use clustine_protocol::codec::{Decode, Reader};
use clustine_protocol::frame::{FrameDecoder, FrameEncoder};
use clustine_protocol::packet_ids;
use clustine_protocol::packets::play::{ClientboundKeepAlive, ServerboundKeepAlive};
use clustine_protocol::packets::{self, Packet};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// How long to wait for the server before a scenario is considered failed.
const TIMEOUT: Duration = Duration::from_secs(30);

/// What `read_frame` says when the server has ended the connection.
const CLOSED: &str = "the server closed the connection";

pub struct Connection {
    reading: Reading,
    /// Shared with the reader task once keep-alives are answered, so that its answers
    /// and everything the connection's owner writes go out as whole frames, one after
    /// the other, each encoded in the order it is sent (the encoder's state is the
    /// connection's, compression included).
    writing: Arc<Mutex<Writing>>,
}

/// The half that reads, either read here as the owner asks for each packet, or by a task
/// of its own that answers keep-alives whether or not anyone reads.
enum Reading {
    Direct {
        half: OwnedReadHalf,
        decoder: FrameDecoder,
    },
    Answering {
        frames: mpsc::UnboundedReceiver<Result<Bytes>>,
        task: JoinHandle<()>,
    },
    /// Only between the two, while the connection is being turned; never seen outside.
    Turning,
}

struct Writing {
    half: OwnedWriteHalf,
    encoder: FrameEncoder,
    /// Whether the reader task answers keep-alives; false only for a client that is to
    /// fall silent.
    answering: bool,
}

impl Writing {
    async fn write<P: Packet>(&mut self, packet: &P) -> Result<()> {
        let mut framed = Vec::new();
        self.encoder.encode(&packets::encode(packet), &mut framed)?;
        self.half.write_all(&framed).await?;
        Ok(())
    }
}

impl Connection {
    /// Connects to `address` (`host:port`).
    pub async fn connect(address: &str) -> Result<Self> {
        let stream = timeout(TIMEOUT, TcpStream::connect(address))
            .await
            .context("connecting timed out")?
            .with_context(|| format!("connecting to {address}"))?;
        stream.set_nodelay(true)?;
        let (read, write) = stream.into_split();
        Ok(Self {
            reading: Reading::Direct {
                half: read,
                decoder: FrameDecoder::new(),
            },
            writing: Arc::new(Mutex::new(Writing {
                half: write,
                encoder: FrameEncoder::new(),
                answering: true,
            })),
        })
    }

    /// Switches to the compressed packet format, as told by the server during login.
    pub fn enable_compression(&mut self, threshold: i32) -> Result<()> {
        let threshold = usize::try_from(threshold).context("negative compression threshold")?;
        let Reading::Direct { decoder, .. } = &mut self.reading else {
            bail!("compression is enabled during login, before keep-alives are answered");
        };
        decoder.enable_compression();
        self.writing
            .try_lock()
            .context("nothing else writes during login")?
            .encoder
            .enable_compression(threshold);
        Ok(())
    }

    /// From now on answers every keep-alive of the play state at once, whether or not
    /// anyone reads the connection: a test that waits on something else (a server's
    /// restart, a killed worker, another client) for longer than the server's 15 seconds
    /// would otherwise have its clients dropped as timed out. The keep-alives are still
    /// passed on to `read_frame`, after their answers, so a reader can count them; it
    /// must not answer them itself, since a server drops a client that answers an id it
    /// is not waiting for. To be called once the connection is in the play state; doing
    /// it again changes nothing.
    pub fn answer_keep_alives(&mut self) {
        if !matches!(self.reading, Reading::Direct { .. }) {
            return;
        }
        let Reading::Direct { half, decoder } =
            std::mem::replace(&mut self.reading, Reading::Turning)
        else {
            unreachable!("the reading was looked at just before");
        };
        let (sender, frames) = mpsc::unbounded_channel();
        let task = tokio::spawn(answer(half, decoder, Arc::clone(&self.writing), sender));
        self.reading = Reading::Answering { frames, task };
    }

    /// Stops answering keep-alives, for a test of a client that falls silent in play:
    /// the server is to drop it as timed out. A keep-alive that came before this may
    /// still have been answered.
    pub async fn stop_answering_keep_alives(&mut self) {
        self.writing.lock().await.answering = false;
    }

    pub async fn write<P: Packet>(&mut self, packet: &P) -> Result<()> {
        self.writing.lock().await.write(packet).await
    }

    /// Ends the connection from this side, as a client does that leaves: the server
    /// reads what was written before this and then the end.
    pub async fn close(&mut self) -> Result<()> {
        self.writing.lock().await.half.shutdown().await?;
        Ok(())
    }

    /// Waits for the next packet and returns it unframed: its id followed by its body.
    pub async fn read_frame(&mut self) -> Result<Bytes> {
        match &mut self.reading {
            Reading::Direct { half, decoder } => read_direct(half, decoder).await,
            Reading::Answering { frames, .. } => timeout(TIMEOUT, frames.recv())
                .await
                .context("the server sent nothing for too long")?
                // The reader task passes on how the connection ended before it ends, so
                // a channel that ends without that is a task that was stopped.
                .unwrap_or_else(|| Err(anyhow!(CLOSED))),
            Reading::Turning => unreachable!("a connection is turned within one call"),
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Reading::Answering { task, .. } = &self.reading {
            task.abort();
        }
    }
}

async fn read_direct(half: &mut OwnedReadHalf, decoder: &mut FrameDecoder) -> Result<Bytes> {
    loop {
        if let Some(frame) = decoder.next_frame()? {
            return Ok(frame);
        }
        let read = timeout(TIMEOUT, half.read_buf(decoder.buffer()))
            .await
            .context("the server sent nothing for too long")??;
        if read == 0 {
            bail!(CLOSED);
        }
    }
}

/// The reader task of a turned connection: reads every frame, answers each keep-alive
/// and passes everything on, until the connection ends (which it passes on too) or
/// nobody is left to pass to.
async fn answer(
    mut half: OwnedReadHalf,
    mut decoder: FrameDecoder,
    writing: Arc<Mutex<Writing>>,
    frames: mpsc::UnboundedSender<Result<Bytes>>,
) {
    loop {
        let frame = loop {
            match decoder.next_frame() {
                Ok(Some(frame)) => break Ok(frame),
                Ok(None) => {}
                Err(error) => break Err(anyhow::Error::from(error)),
            }
            // No limit on how long the server may stay silent: the limit is the reader's,
            // in `read_frame`, which applies however the frames arrive.
            match half.read_buf(decoder.buffer()).await {
                Ok(0) => break Err(anyhow!(CLOSED)),
                Ok(_) => {}
                Err(error) => break Err(error.into()),
            }
        };
        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => {
                let _ = frames.send(Err(error));
                return;
            }
        };
        if let Some(id) = keep_alive(&frame) {
            // A failed answer is a connection that is ending: reading on says how, as
            // `read_frame` would have said had it been read directly.
            let mut writing = writing.lock().await;
            if writing.answering {
                let _ = writing.write(&ServerboundKeepAlive { id }).await;
            }
        }
        if frames.send(Ok(frame)).is_err() {
            return;
        }
    }
}

/// The id of a play keep-alive, if `frame` is one.
fn keep_alive(frame: &[u8]) -> Option<i64> {
    let mut reader = Reader::new(frame);
    if reader.var_int().ok()? != packet_ids::play::clientbound::KEEP_ALIVE {
        return None;
    }
    ClientboundKeepAlive::decode(&mut reader)
        .ok()
        .map(|packet| packet.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// Plays a server's side of an uncompressed connection: frames and reads packets.
    struct Server {
        stream: TcpStream,
        decoder: FrameDecoder,
    }

    impl Server {
        async fn send<P: Packet>(&mut self, packet: &P) {
            let mut framed = Vec::new();
            FrameEncoder::new()
                .encode(&packets::encode(packet), &mut framed)
                .unwrap();
            self.stream.write_all(&framed).await.unwrap();
        }

        async fn receive(&mut self) -> Bytes {
            read_direct_stream(&mut self.stream, &mut self.decoder)
                .await
                .unwrap()
        }
    }

    async fn read_direct_stream(
        stream: &mut TcpStream,
        decoder: &mut FrameDecoder,
    ) -> Result<Bytes> {
        loop {
            if let Some(frame) = decoder.next_frame()? {
                return Ok(frame);
            }
            if stream.read_buf(decoder.buffer()).await? == 0 {
                bail!(CLOSED);
            }
        }
    }

    async fn connected() -> (Connection, Server) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let (connection, accepted) = tokio::join!(Connection::connect(&address), listener.accept());
        let server = Server {
            stream: accepted.unwrap().0,
            decoder: FrameDecoder::new(),
        };
        (connection.unwrap(), server)
    }

    #[tokio::test]
    async fn a_turned_connection_answers_a_keep_alive_nobody_reads_and_passes_it_on_after() {
        let (mut connection, mut server) = connected().await;
        connection.answer_keep_alives();

        server.send(&ClientboundKeepAlive { id: 42 }).await;
        // Nobody reads the connection, yet the answer comes.
        let answer = server.receive().await;
        let mut reader = Reader::new(&answer);
        assert_eq!(
            reader.var_int().unwrap(),
            packet_ids::play::serverbound::KEEP_ALIVE
        );
        assert_eq!(ServerboundKeepAlive::decode(&mut reader).unwrap().id, 42);

        // The keep-alive still reaches the reader, after its answer.
        assert_eq!(
            keep_alive(&connection.read_frame().await.unwrap()),
            Some(42)
        );
    }

    #[tokio::test]
    async fn a_turned_connection_writes_between_its_answers_and_says_when_the_server_closes() {
        let (mut connection, mut server) = connected().await;
        connection.answer_keep_alives();

        connection
            .write(&ServerboundKeepAlive { id: 7 })
            .await
            .unwrap();
        let written = server.receive().await;
        let mut reader = Reader::new(&written);
        reader.var_int().unwrap();
        assert_eq!(ServerboundKeepAlive::decode(&mut reader).unwrap().id, 7);

        drop(server);
        let error = connection.read_frame().await.unwrap_err();
        assert_eq!(error.to_string(), CLOSED);
    }
}
